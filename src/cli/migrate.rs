//! `berbr migrate`: task notes move out of an Obsidian vault into berbr's own
//! folder, `~/.berbr/projects/<project>/tasks` (closed ones in `done/`). Copies
//! only: the vault keeps every note until the person deletes it, and going back
//! is removing `~/.berbr/config.toml`.

use crate::config::{self, Config};
use crate::vault::note::Note;
use serde_yaml::Value;
use std::path::{Path, PathBuf};

#[derive(Default)]
struct Report {
    copied: usize,
    closed: usize,
    skipped: Vec<String>,
}

pub(super) fn run(projects: &[Config], dry_run: bool) -> Result<(), String> {
    let source = config::config_path();
    let target_config = config::berbr_home().join("config.toml");
    let vaulted: Vec<&Config> = projects.iter().filter(|p| !p.vault.own).collect();
    if vaulted.is_empty() {
        println!("Tasks already live in {}", config::berbr_home().display());
        return Ok(());
    }
    let text = std::fs::read_to_string(&source)
        .map_err(|e| format!("can't read {}: {e}", source.display()))?;
    let own_config = config::without_vault(&text).map_err(|e| {
        format!(
            "{}: {e}; drop [vault] and task_dirs by hand into {}",
            source.display(),
            target_config.display()
        )
    })?;
    let _lock = crate::vault::lock().map_err(|e| e.to_string())?;
    let mut report = Report::default();
    for project in vaulted {
        migrate_project(project, dry_run, &mut report)?;
    }
    let verb = if dry_run { "Would copy" } else { "Copied" };
    println!(
        "{verb} {} task(s), {} closed into done/",
        report.copied, report.closed
    );
    for skipped in &report.skipped {
        println!("  skipped {skipped}");
    }
    if target_config.exists() {
        println!("{} exists; left as it is", target_config.display());
    } else if dry_run {
        println!(
            "Would write {} from {}",
            target_config.display(),
            source.display()
        );
    } else {
        write_new(&target_config, &own_config)?;
        println!(
            "Wrote {} from {} (kept)",
            target_config.display(),
            source.display()
        );
    }
    if !dry_run {
        println!(
            "The vault's notes are untouched: delete them once this looks right. To go back, remove {}.",
            target_config.display()
        );
    }
    Ok(())
}

fn migrate_project(project: &Config, dry_run: bool, report: &mut Report) -> Result<(), String> {
    let tasks_dir = config::project_home(&project.name).join("tasks");
    let (tasks, skipped) = crate::vault::task::load_tasks(&project.vault);
    report
        .skipped
        .extend(skipped.into_iter().map(|e| format!("{e} (unreadable)")));
    for task in tasks {
        let from = task.note.path.clone();
        let Some(name) = from.file_name() else {
            continue;
        };
        let to = if task.done {
            tasks_dir.join("done").join(name)
        } else {
            tasks_dir.join(name)
        };
        if to.exists() {
            report
                .skipped
                .push(format!("{} (already there)", to.display()));
            continue;
        }
        report.copied += 1;
        report.closed += usize::from(task.done);
        if dry_run {
            continue;
        }
        copy_note(&from, &to, task.created.as_deref())?;
    }
    copy_counter(project, &tasks_dir, dry_run)
}

/// The note as it is, plus `created:` from where it was found: a copy's own
/// file dates and the vault's git history no longer tell.
fn copy_note(from: &Path, to: &Path, created: Option<&str>) -> Result<(), String> {
    let parent = to.parent().unwrap_or(to);
    std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    std::fs::copy(from, to).map_err(|e| format!("{} → {}: {e}", from.display(), to.display()))?;
    let mut note = Note::read(to).map_err(|e| e.to_string())?;
    if note.string("created").is_none()
        && let Some(created) = created
    {
        note.set("created", Value::String(created.to_string()));
        note.save().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Task numbers go on from where the vault left them.
fn copy_counter(project: &Config, tasks_dir: &Path, dry_run: bool) -> Result<(), String> {
    let name = format!("{}.next", project.name.trim().to_ascii_lowercase());
    let from = project.vault.path.join(".worktable").join(&name);
    let to: PathBuf = tasks_dir
        .parent()
        .unwrap_or(tasks_dir)
        .join(".worktable")
        .join(&name);
    if dry_run || !from.is_file() || to.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(to.parent().unwrap_or(&to)).map_err(|e| e.to_string())?;
    std::fs::copy(&from, &to)
        .map(|_| ())
        .map_err(|e| format!("{}: {e}", to.display()))
}

fn write_new(path: &Path, text: &str) -> Result<(), String> {
    std::fs::create_dir_all(path.parent().unwrap_or(path)).map_err(|e| e.to_string())?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, text.as_bytes()))
        .map_err(|e| format!("can't write {}: {e}", path.display()))
}
