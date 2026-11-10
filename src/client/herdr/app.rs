//! Adapter for the desktop Herdr App and its separate herdr-core backend.
//! Keep the legacy client contract so CLI and TUI can use either backend.
use super::{HerdrError, Result};
use serde_json::{Value, json};
use std::cell::RefCell;
use std::process::Command;
use std::sync::{Arc, Mutex};

type Answer = std::result::Result<Vec<Value>, String>;
#[derive(Default)]
struct Inventory {
    spaces: Option<Answer>,
    agents: Option<Answer>,
}
type Context = Arc<Mutex<Inventory>>;
thread_local! { static CURRENT: RefCell<Option<Context>> = const { RefCell::new(None) }; }

pub(crate) struct Operation {
    previous: Option<Context>,
}
impl Drop for Operation {
    fn drop(&mut self) {
        CURRENT.with(|slot| *slot.borrow_mut() = self.previous.take());
    }
}
pub(crate) fn operation() -> Operation {
    let previous = CURRENT.with(|slot| {
        let old = slot.borrow().clone();
        if old.is_none() {
            *slot.borrow_mut() = Some(Arc::default());
        }
        old
    });
    Operation { previous }
}

pub(crate) fn inherit<R: Send>(f: impl FnOnce() -> R + Send) -> impl FnOnce() -> R + Send {
    let context = CURRENT.with(|slot| slot.borrow().clone());
    move || {
        let previous = CURRENT.with(|slot| slot.replace(context));
        let _scope = Operation { previous };
        f()
    }
}

fn invalidate() {
    CURRENT.with(|slot| {
        if let Some(cache) = slot.borrow().as_ref() {
            *cache.lock().unwrap_or_else(|e| e.into_inner()) = Inventory::default();
        }
    });
}

fn cached(agents: bool, fetch: impl FnOnce() -> Result<Vec<Value>>) -> Result<Vec<Value>> {
    let context = CURRENT.with(|slot| slot.borrow().clone());
    let Some(context) = context else {
        return fetch();
    };
    let mut cache = context.lock().unwrap_or_else(|e| e.into_inner());
    let value = if agents {
        &mut cache.agents
    } else {
        &mut cache.spaces
    };
    value
        .get_or_insert_with(|| fetch().map_err(|e| e.to_string()))
        .clone()
        .map_err(HerdrError::NonZeroExit)
}

pub(super) fn enabled() -> bool {
    enabled_from_env(
        std::env::var("BERBR_BACKEND").ok().as_deref(),
        std::env::var("HC_TERMINAL_SOCKET").ok().as_deref(),
        std::env::var("HERDR_ENV").ok().as_deref(),
    )
}

fn enabled_from_env(backend: Option<&str>, socket: Option<&str>, herdr_env: Option<&str>) -> bool {
    backend == Some("herdr-app")
        || (socket.is_some_and(|s| !s.is_empty()) && herdr_env != Some("1"))
}

fn output_of(bin: &str, args: &[&str]) -> Result<Vec<u8>> {
    let out = crate::log::command(bin, Command::new(bin).args(args))?;
    if !out.status.success() {
        return Err(HerdrError::NonZeroExit(format!(
            "{bin}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    Ok(out.stdout)
}

fn read(bin: &str, args: &[&str]) -> Result<Value> {
    Ok(serde_json::from_slice(&output_of(bin, args)?)?)
}

fn act(command: &str, args: Value) -> Result<Value> {
    let result = read("herdr-app", &["do", command, &args.to_string()]);
    if matches!(
        command,
        "space.new"
            | "tab.new"
            | "pane.split"
            | "space.rename"
            | "tab.rename"
            | "space.close"
            | "tab.close"
            | "pane.close"
    ) {
        invalidate();
    }
    result
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn spaces() -> Result<Vec<Value>> {
    cached(false, read_spaces)
}

fn read_spaces() -> Result<Vec<Value>> {
    let state = read("herdr-app", &["ls"])?;
    let spaces = state
        .get("spaces")
        .and_then(Value::as_array)
        .ok_or_else(|| HerdrError::NonZeroExit("herdr-app ls: missing spaces".into()))?;
    // Local projects only. Never query remote machines just to draw a board.
    Ok(spaces
        .iter()
        .filter(|s| s["machine"].is_null())
        .cloned()
        .collect())
}

fn tabs(space: &Value) -> impl Iterator<Item = &Value> {
    space["tabs"].as_array().into_iter().flatten()
}

fn terminals(tab: &Value) -> impl Iterator<Item = &Value> {
    tab["terminals"].as_array().into_iter().flatten()
}

fn agents() -> Result<Vec<Value>> {
    let spaces = spaces()?;
    cached(true, || {
        let jobs: Vec<_> = spaces
            .iter()
            .flat_map(|space| {
                tabs(space).flat_map(move |tab| {
                    terminals(tab)
                        .filter(|t| t["app"].as_str().is_some())
                        .map(move |term| (space.clone(), tab.clone(), term.clone()))
                })
            })
            .collect();
        let jobs = match read("hc", &["ls", "--json"]) {
            Ok(inventory) => with_child_agents(jobs, &inventory),
            Err(_) => jobs,
        };
        super::super::parallel(&jobs, 8, |(space, tab, term)| {
            let address = text(term, "terminal");
            let detail = read("hc", &["get", address, "--json"])?;
            let sessions = open_codex_sessions(&detail, text(term, "app"));
            Ok(json!({
                "name": detail["labels"]["berbr.name"].as_str().or(detail["name"].as_str()),
                "agent_status": term["status"], "cwd": detail["foreground"]["cwd"].as_str().or(detail["cwd"].as_str()),
                "pane_id": address, "tab_id": tab["id"], "workspace_id": space["id"],
                "agent": term["app"], "agent_session": session_arg(&detail).or_else(|| sessions.first().cloned()).map(|id| json!({"value":id})),
                "session_aliases":sessions,
                "created_ms":detail["created_ms"],
                "role":terminal_role(&detail), "parent":detail["parent"],
                "role_explicit":matches!(detail["labels"]["berbr.role"].as_str(), Some("primary" | "helper" | "reviewer" | "temporary")),
                "has_pane":term["_berbr_child"] != true,
                "berbr_task": detail["labels"]["berbr.task"]
            }))
        }).into_iter().zip(&jobs).filter_map(|(result, (_, _, term))| {
            let result = result.unwrap_or_else(|| Err(HerdrError::NonZeroExit("hc detail read panicked".into())));
            if term["_berbr_child"] == true && result.is_err() { None } else { Some(result) }
        }).collect()
    })
}

/// Child terminals may have no pane of their own. Keep their real ancestry so
/// sessions can be classified without using their title or agent name.
fn with_child_agents(
    mut jobs: Vec<(Value, Value, Value)>,
    inventory: &Value,
) -> Vec<(Value, Value, Value)> {
    let mut places: std::collections::HashMap<String, (Value, Value)> = jobs
        .iter()
        .map(|(space, tab, term)| {
            (
                text(term, "terminal").to_string(),
                (space.clone(), tab.clone()),
            )
        })
        .collect();
    loop {
        let mut added = false;
        for child in inventory["terminals"].as_array().into_iter().flatten() {
            let address = text(child, "address");
            if address.is_empty()
                || places.contains_key(address)
                || child["state"] != "live"
                || !matches!(
                    child["agent"].as_str(),
                    Some("claude" | "claude-code" | "codex")
                )
            {
                continue;
            }
            let Some((space, tab)) = places.get(text(child, "parent")).cloned() else {
                continue;
            };
            places.insert(address.into(), (space.clone(), tab.clone()));
            jobs.push((
                space,
                tab,
                json!({"terminal":address, "app":child["agent"],
                "status":child["status"], "_berbr_child":true}),
            ));
            added = true;
        }
        if !added {
            return jobs;
        }
    }
}

fn terminal_role(detail: &Value) -> Option<&'static str> {
    match detail["labels"]["berbr.role"].as_str() {
        Some("primary") => return Some("primary"),
        Some("helper" | "reviewer" | "temporary") => return Some("helper"),
        _ => {}
    }
    if detail["parent"]
        .as_str()
        .is_some_and(|parent| !parent.is_empty())
    {
        Some("helper")
    } else if detail.get("parent").is_some_and(Value::is_null) {
        Some("primary")
    } else {
        None
    }
}

/// Resumed conversations name their exact ID in the owning CLI's argv. Fresh
/// agents without an ID are reconciled with their worktree's recent sessions.
fn session_arg(detail: &Value) -> Option<String> {
    if let Some(id) = detail["labels"]["berbr.session"]
        .as_str()
        .filter(|id| !id.is_empty())
    {
        return Some(crate::client::agent::AgentKind::native_id(id).into());
    }
    let args = detail["foreground"]["argv"]
        .as_array()
        .or_else(|| detail["argv"].as_array())?;
    args.windows(2).find_map(|pair| {
        let flag = pair[0].as_str()?;
        let id = pair[1].as_str()?;
        (["--resume", "-r", "--session-id", "resume"].contains(&flag)
            && !id.is_empty()
            && !id.starts_with('-'))
        .then(|| id.to_string())
    })
}

/// Codex --no-daemon keeps its rollout files open. Compaction can change the
/// rollout ID without restarting the terminal, so retain all runtime aliases.
fn open_codex_sessions(detail: &Value, app: &str) -> Vec<String> {
    use std::io::{BufRead, Read};
    if app != "codex" {
        return vec![];
    }
    let Some(pid) = detail["foreground"]["pid"].as_u64() else {
        return vec![];
    };
    let root = std::env::var_os("CODEX_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::home_dir().map(|p| p.join(".codex")))
        .map(|p| p.join("sessions"));
    let Some(root) = root else {
        return vec![];
    };
    let Ok(output) = Command::new("lsof")
        .args(["-a", "-p", &pid.to_string(), "-Fn"])
        .output()
    else {
        return vec![];
    };
    let mut ids = vec![];
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some(path) = line
            .strip_prefix('n')
            .map(std::path::Path::new)
            .filter(|path| {
                path.starts_with(&root) && path.extension().is_some_and(|ext| ext == "jsonl")
            })
        else {
            continue;
        };
        let Ok(file) = std::fs::File::open(path) else {
            continue;
        };
        let mut first = String::new();
        if std::io::BufReader::new(file)
            .take(65_536)
            .read_line(&mut first)
            .is_err()
        {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&first) else {
            continue;
        };
        if value["type"] != "session_meta" {
            continue;
        }
        if let Some(id) = value["payload"]["id"].as_str()
            && !ids.iter().any(|known| known == id)
        {
            ids.push(id.to_string());
        }
    }
    ids
}

/// Where each space is now, as the app's sidebar sees it: the live folder of
/// its first terminal, else the folder it was made in; with that terminal.
pub(super) fn space_places() -> Result<Vec<(String, String, Option<String>)>> {
    Ok(spaces()?
        .iter()
        .map(|space| {
            let live = tabs(space).flat_map(terminals).find_map(|term| {
                let address = text(term, "terminal");
                let detail = read("hc", &["get", address, "--json"]).ok()?;
                detail["foreground"]["cwd"]
                    .as_str()
                    .or(detail["cwd"].as_str())
                    .filter(|cwd| !cwd.is_empty())
                    .map(|cwd| (cwd.to_string(), address.to_string()))
            });
            let (place, terminal) = match live {
                Some((cwd, address)) => (cwd, Some(address)),
                None => (text(space, "folder").to_string(), None),
            };
            (text(space, "id").to_string(), place, terminal)
        })
        .collect())
}

pub(super) fn focus_known(space: &str, tab: &str, terminal: &str) -> Result<()> {
    act("space.select", json!({"id":space}))?;
    act("tab.select", json!({"id":tab}))?;
    act("agent.focus", json!({"terminal":terminal}))?;
    Ok(())
}

fn agent(target: &str) -> Result<Value> {
    let found: Vec<_> = agents()?
        .into_iter()
        .filter(|a| text(a, "pane_id") == target || text(a, "name") == target)
        .collect();
    match found.as_slice() {
        [agent] => Ok(agent.clone()),
        _ => Err(HerdrError::NonZeroExit(format!(
            "agent {target}: missing or ambiguous"
        ))),
    }
}

pub(super) fn tab_of(address: &str) -> Result<Option<String>> {
    for space in spaces()? {
        for tab in tabs(&space) {
            if terminals(tab)
                .any(|term| text(term, "terminal") == address || text(term, "pane") == address)
            {
                return Ok(Some(text(tab, "id").to_string()));
            }
        }
    }
    Ok(None)
}

fn pane_of(address: &str) -> Result<String> {
    for space in spaces()? {
        for tab in tabs(&space) {
            for term in terminals(tab) {
                if text(term, "terminal") == address {
                    return Ok(text(term, "pane").to_string());
                }
            }
        }
    }
    Err(HerdrError::NonZeroExit(format!(
        "no app pane for {address}"
    )))
}

fn arg<'a>(args: &'a [&str], name: &str) -> Result<&'a str> {
    args.windows(2)
        .find(|a| a[0] == name)
        .map(|a| a[1])
        .ok_or_else(|| HerdrError::NonZeroExit(format!("missing {name}")))
}

/// App commands focus new terminals. Restore the caller after background creation.
fn create(command: &str, args: Value) -> Result<Value> {
    let before = read("herdr-app", &["status"])?;
    let result = act(command, args)?;
    if before["space"].as_str().is_some() {
        act("space.select", json!({"id": before["space"]}))?;
        act("tab.select", json!({"id": before["tab"]}))?;
        if before["focused_pane"].as_str().is_some() {
            act("pane.focus", json!({"pane": before["focused_pane"]}))?;
        }
    }
    Ok(result)
}

pub(super) fn output(args: &[&str]) -> Result<Vec<u8>> {
    let result = match args {
        ["agent", "list"] => json!({"agents": agents()?}),
        ["workspace", "list"] => json!({"workspaces": spaces()?.iter().map(|s| json!({
            "workspace_id": s["id"], "label": s["name"], "worktree": null
        })).collect::<Vec<_>>()}),
        ["tab", "list"] => json!({"tabs": spaces()?.iter().flat_map(tabs).map(|t| json!({
            "tab_id": t["id"], "label": t["name"]
        })).collect::<Vec<_>>()}),
        ["workspace", "create", rest @ ..] => {
            let r = create(
                "space.new",
                json!({"name": arg(rest, "--label")?, "folder": arg(rest, "--cwd")?, "machine": ""}),
            )?;
            json!({"tab":{"tab_id": r["tab"]},"root_pane":{"pane_id": r["terminal"]}})
        }
        ["tab", "create", rest @ ..] => {
            let space = arg(rest, "--workspace")?;
            if space.is_empty() {
                return Err(HerdrError::NonZeroExit("no task space for new tab".into()));
            }
            let r = create(
                "tab.new",
                json!({"space": space, "name": arg(rest, "--label")?, "folder": arg(rest, "--cwd")?}),
            )?;
            json!({"tab":{"tab_id": r["tab"]},"root_pane":{"pane_id": r["terminal"]}})
        }
        ["pane", "split", rest @ ..] => {
            let pane = pane_of(arg(rest, "--pane")?)?;
            let r = create(
                "pane.split",
                json!({"pane":pane,"direction":arg(rest,"--direction")?}),
            )?;
            json!({"pane":{"pane_id":r["terminal"]}})
        }
        ["pane", "run", address, command] => {
            // Only shells we just created may receive commands, never an active agent.
            let mut t = read("hc", &["get", address, "--json"])?;
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
            // App creation acknowledges before the shell has a foreground PID.
            // Wait only for that startup gap; a known busy process is an error.
            while t["foreground"].is_null()
                && t["status"].is_null()
                && std::time::Instant::now() < deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(50));
                t = read("hc", &["get", address, "--json"])?;
            }
            let foreground = text(&t["foreground"], "name");
            if !["sh", "bash", "zsh"].contains(&foreground) || !t["status"].is_null() {
                return Err(HerdrError::NonZeroExit(format!(
                    "{address} is not an available shell (foreground: {foreground}, status: {})",
                    t["status"]
                )));
            }
            let command = format!(
                "export BERBR_BACKEND=herdr-app BERBR_CONFIG={}; {command}",
                super::shell_quote(&crate::config::config_path().to_string_lossy())
            );
            let sent = output_of("hc", &["send", address, &command]);
            invalidate();
            sent?;
            json!({})
        }
        ["pane", "label", address, label] => {
            let labelled = output_of("hc", &["label", address, label]);
            invalidate();
            labelled?;
            json!({})
        }
        ["pane", "close", address] => {
            let pane = pane_of(address)?;
            act("pane.close", json!({"pane": pane}))?
        }
        ["agent", "rename", address, name] => {
            let renamed = output_of("hc", &["label", address, &format!("berbr.name={name}")]);
            invalidate();
            renamed?;
            json!({})
        }
        ["agent", "get", target] => json!({"agent": agent(target)?}),
        ["agent", "focus", target] => {
            let a = agent(target)?;
            act("agent.focus", json!({"terminal":a["pane_id"]}))?
        }
        ["agent", "read", target, rest @ ..] => {
            let a = agent(target)?;
            return output_of(
                "hc",
                &[
                    "read",
                    text(&a, "pane_id"),
                    "--lines",
                    arg(rest, "--lines")?,
                ],
            );
        }
        ["workspace", "focus", id] => act("space.select", json!({"id":id}))?,
        ["tab", "focus", id] => act("tab.select", json!({"id":id}))?,
        ["workspace", "rename", id, label] => act("space.rename", json!({"id":id,"name":label}))?,
        ["tab", "rename", id, label] => act("tab.rename", json!({"id":id,"name":label}))?,
        ["workspace", "close", id] => act("space.close", json!({"id":id,"confirmed":true}))?,
        ["tab", "close", id] => act("tab.close", json!({"id":id,"confirmed":true}))?,
        _ => {
            return Err(HerdrError::NonZeroExit(format!(
                "unsupported Herdr App operation: {}",
                args.join(" ")
            )));
        }
    };
    Ok(serde_json::to_vec(&json!({"result":result}))?)
}

#[cfg(test)]
mod tests {
    use super::enabled_from_env;

    #[test]
    fn backend_detection_supports_manual_app_terminals_and_preserves_legacy_tui() {
        for (backend, socket, herdr_env, expected) in [
            (None, None, None, false),
            (None, Some(""), None, false),
            (None, Some("/tmp/demo-core.sock"), None, true),
            (None, Some("/tmp/demo-core.sock"), Some("0"), true),
            (None, Some("/tmp/demo-core.sock"), Some("1"), false),
            (None, None, Some("1"), false),
            (Some("other"), None, None, false),
            (Some("herdr-app"), None, None, true),
            (Some("herdr-app"), Some(""), Some("1"), true),
        ] {
            assert_eq!(
                enabled_from_env(backend, socket, herdr_env),
                expected,
                "backend={backend:?}, socket={socket:?}, HERDR_ENV={herdr_env:?}"
            );
        }
    }
}

#[cfg(test)]
mod inventory_tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn operation_inventory_is_shared_with_workers_invalidated_and_not_reused_later() {
        let calls = AtomicUsize::new(0);
        {
            let _scope = operation();
            let fetch = || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(vec![json!({"id":"demo"})])
            };
            cached(false, fetch).unwrap();
            std::thread::scope(|scope| {
                scope
                    .spawn(inherit(|| cached(false, fetch).unwrap()))
                    .join()
                    .unwrap();
            });
            assert_eq!(calls.load(Ordering::SeqCst), 1);
            invalidate();
            cached(false, fetch).unwrap();
            assert_eq!(calls.load(Ordering::SeqCst), 2);
        }
        let _scope = operation();
        cached(false, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        })
        .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }
    #[test]
    fn explicit_resume_ids_come_from_the_foreground_cli_or_a_saved_label() {
        use serde_json::json;
        assert_eq!(
            super::session_arg(
                &json!({"foreground":{"argv":["codex","--no-daemon","resume","old-id"]}})
            ),
            Some("old-id".into())
        );
        assert_eq!(
            super::session_arg(&json!({"argv":["claude","--resume","specific-id"]})),
            Some("specific-id".into())
        );
        assert_eq!(
            super::session_arg(&json!({"argv":["codex","resume","--last"]})),
            None
        );
        assert_eq!(
            super::session_arg(&json!({"labels":{"berbr.session":"codex:known"}})),
            Some("known".into())
        );
    }
    #[test]
    fn roles_use_parent_and_explicit_metadata_never_the_title() {
        use serde_json::json;
        assert_eq!(
            super::terminal_role(&json!({"parent":"root","title":"Implement feature"})),
            Some("helper")
        );
        assert_eq!(
            super::terminal_role(&json!({"parent":null,"title":"Review changes"})),
            Some("primary")
        );
        assert_eq!(
            super::terminal_role(&json!({"parent":null,"labels":{"berbr.role":"reviewer"}})),
            Some("helper")
        );
        assert_eq!(
            super::terminal_role(&json!({"parent":"root","labels":{"berbr.role":"primary"}})),
            Some("primary")
        );
        assert_eq!(super::terminal_role(&json!({"title":"reviewer"})), None);
    }

    #[test]
    fn child_inventory_keeps_ancestry_but_not_unrelated_or_finished_terminals() {
        use serde_json::json;
        let jobs = vec![(
            json!({"id":"space"}),
            json!({"id":"tab"}),
            json!({"terminal":"root","app":"codex"}),
        )];
        let all = json!({"terminals":[
            {"address":"grandchild","parent":"child","state":"live","agent":"codex","status":"idle"},
            {"address":"child","parent":"root","state":"live","agent":"claude-code","status":"working"},
            {"address":"unrelated","parent":"other","state":"live","agent":"codex"},
            {"address":"finished","parent":"root","state":"finished","agent":"codex"}]});
        let jobs = super::with_child_agents(jobs, &all);
        assert_eq!(jobs.len(), 3);
        assert_eq!(jobs[1].2["terminal"], "child");
        assert_eq!(jobs[2].2["terminal"], "grandchild");
        assert!(
            jobs.iter()
                .all(|(space, tab, _)| space["id"] == "space" && tab["id"] == "tab")
        );
    }
}
