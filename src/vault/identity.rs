//! Project-local task numbers, persisted in the vault so deleting a note or
//! clearing a cache cannot reuse an identity. Call while holding the vault lock.

use super::note::Note;
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
    // Recheck only filenames under the allocation lock. Creation dates and
    // parsed task metadata were already loaded by start; they cannot allocate IDs.
    let prefix = format!("{project}-");
    let mut next = saved;
    let mut dirs = config.vault.task_dirs.clone();
    dirs.push(config.vault.new_task_dir.clone());
    dirs.sort();
    dirs.dedup();
    for dir in dirs {
        for path in note_paths(&config.vault.path.join(dir))? {
            if let Some(n) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.strip_prefix(&prefix))
                .and_then(|s| s.parse::<u64>().ok())
            {
                next = next.max(n.checked_add(1).ok_or("task counter exhausted")?);
            }
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
    keys.push(("created", Value::String(chrono::Local::now().to_rfc3339())));
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

fn note_paths(dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if kind.is_dir() {
            paths.extend(note_paths(&entry.path())?);
        } else if kind.is_file() && entry.path().extension().is_some_and(|e| e == "md") {
            paths.push(entry.path());
        }
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::VaultConfig;

    #[test]
    fn locked_filename_inventory_prevents_reuse_without_loading_note_metadata() {
        let root = crate::table::fixtures::temp_dir();
        std::fs::create_dir_all(root.join("tasks")).unwrap();
        std::fs::write(root.join("tasks/demo-42.md"), "---\nstate: [\n---\n").unwrap();
        let config = Config {
            name: "demo".into(),
            jira: None,
            gitlab: None,
            github: None,
            workflow: Default::default(),
            repos: vec![],
            vault: VaultConfig {
                path: root,
                task_dirs: vec!["tasks".into()],
                new_task_dir: "tasks".into(),
                own: false,
            },
        };
        let ids = std::thread::scope(|scope| {
            (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        let _lock = crate::vault::lock().unwrap();
                        create(&config, "Synthetic", &[], "").unwrap().name()
                    })
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|job| job.join().unwrap())
                .collect::<std::collections::HashSet<_>>()
        });
        assert_eq!(ids.len(), 8);
        assert!(ids.contains("demo-43"));
        assert!(ids.contains("demo-50"));
    }
}
