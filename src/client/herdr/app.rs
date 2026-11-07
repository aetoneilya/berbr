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
                        .map(move |term| (space, tab, term))
                })
            })
            .collect();
        super::super::parallel(&jobs, 8, |(space, tab, term)| {
            let address = text(term, "terminal");
            let detail = read("hc", &["get", address, "--json"])?;
            Ok(json!({
                "name": detail["labels"]["berbr.name"].as_str().or(detail["name"].as_str()),
                "agent_status": term["status"], "cwd": detail["foreground"]["cwd"].as_str().or(detail["cwd"].as_str()),
                "pane_id": address, "tab_id": tab["id"], "workspace_id": space["id"],
                "agent": term["app"], "agent_session": null
            }))
        }).into_iter().map(|result| result.unwrap_or_else(|| Err(HerdrError::NonZeroExit("hc detail read panicked".into())))).collect()
    })
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
}
