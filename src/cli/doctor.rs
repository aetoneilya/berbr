use serde_json::json;
use std::process::Command;

pub(super) fn run() -> Result<(), String> {
    let programs: Vec<_> = ["git", "herdr-app", "hc", "zsh", "codex", "claude", "gh"].into_iter().map(|program| {
        let installed = Command::new("/bin/sh").args(["-c", "command -v \"$1\" >/dev/null 2>&1", "berbr-check", program]).status().is_ok_and(|s|s.success());
        json!({"program":program,"installed":installed,"required":matches!(program,"git"|"herdr-app"|"hc"|"zsh")})
    }).collect();
    let (config, error) = match crate::config::load_projects() {
        Ok(projects) => (
            json!(projects.iter().map(|p| &p.name).collect::<Vec<_>>()),
            None,
        ),
        Err(e) => (json!([]), Some(e.to_string())),
    };
    println!(
        "{}",
        json!({"programs":programs,"projects":config,"config_error":error,"config_path":crate::config::config_path(),"next":"Connect a repository: berbr init --repo <path> --agent codex. For GitHub authentication: gh auth login --hostname github.com."})
    );
    Ok(())
}
