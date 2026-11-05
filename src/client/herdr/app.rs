//! Adapter for the desktop Herdr App and its separate herdr-core backend.
//! Keep the legacy client contract so CLI and TUI can use either backend.
use super::{HerdrError, Result};
use serde_json::{Value, json};
use std::process::Command;

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
    let out = Command::new(bin).args(args).output()?;
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
    read("herdr-app", &["do", command, &args.to_string()])
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

fn spaces() -> Result<Vec<Value>> {
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
    let mut agents = Vec::new();
    for space in spaces()? {
        for tab in tabs(&space) {
            for term in terminals(tab).filter(|t| t["app"].as_str().is_some()) {
                let address = text(term, "terminal");
                let detail = read("hc", &["get", address, "--json"])?;
                agents.push(json!({
                    "name": detail["labels"]["berbr.name"].as_str().or(detail["name"].as_str()),
                    "agent_status": term["status"], "cwd": detail["foreground"]["cwd"].as_str().or(detail["cwd"].as_str()),
                    "pane_id": address, "tab_id": tab["id"], "workspace_id": space["id"],
                    "agent": term["app"], "agent_session": null
                }));
            }
        }
    }
    Ok(agents)
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
            output_of("hc", &["send", address, &command])?;
            json!({})
        }
        ["agent", "rename", address, name] => {
            output_of("hc", &["label", address, &format!("berbr.name={name}")])?;
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
