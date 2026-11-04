//! Project-local task numbers, persisted in the vault so deleting a note or
//! clearing a cache cannot reuse an identity. Call while holding the vault lock.

use super::note::Note;
use super::task::load_tasks;
use crate::config::Config;
use serde_yaml::Value;

pub fn create(
    config: &Config,
    title: &str,
    keys: &[(&str, Value)],
    body: &str,
) -> Result<Note, String> {
    let project = config.name.trim().to_ascii_lowercase();
    if project.is_empty()
        || !project
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
    {
        return Err(
            "project names used in task IDs must contain Latin letters, digits, '-' or '_'".into(),
        );
    }
    let counter = config
        .vault
        .path
        .join(".worktable")
        .join(format!("{project}.next"));
    let saved = match std::fs::read_to_string(&counter) {
        Ok(s) => s
            .trim()
            .parse::<u64>()
            .map_err(|e| format!("{}: {e}", counter.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 0,
        Err(e) => return Err(e.to_string()),
    };
    let (tasks, errors) = load_tasks(&config.vault);
    if let Some(error) = errors.first() {
        return Err(error.to_string());
    }
    let prefix = format!("{project}-");
    let mut next = saved;
    for task in tasks {
        if let Some(n) = task
            .name
            .strip_prefix(&prefix)
            .and_then(|s| s.parse::<u64>().ok())
        {
            next = next.max(n.checked_add(1).ok_or("task counter exhausted")?);
        }
    }
    let id = format!("{prefix}{next}");
    let following = next.checked_add(1).ok_or("task counter exhausted")?;
    std::fs::create_dir_all(counter.parent().unwrap()).map_err(|e| e.to_string())?;
    let tmp = counter.with_extension("tmp");
    std::fs::write(&tmp, following.to_string()).map_err(|e| e.to_string())?;
    std::fs::rename(tmp, counter).map_err(|e| e.to_string())?;
    let mut keys = keys.to_vec();
    keys.push(("id", Value::String(id.clone())));
    keys.push(("project", Value::String(config.name.clone())));
    if !title.trim().is_empty() {
        keys.push(("title", Value::String(title.trim().into())));
    }
    let path = config
        .vault
        .path
        .join(&config.vault.new_task_dir)
        .join(format!("{id}.md"));
    Note::create(&path, &keys, body).map_err(|e| e.to_string())
}
