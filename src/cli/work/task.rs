//! Task context tools and findings appended to task notes.

use super::common::{current_task, err, find_task, name_from_text};
use crate::cli::{TaskCommand, load};
use crate::client::{herdr, worktree};
use crate::config::Config;
use crate::table;
use crate::vault::note::NoteError;
use crate::vault::task::{Task, same_name};
use std::io::Read;
use std::path::{Path, PathBuf};

pub(in crate::cli) fn cli_task(config: &Config, command: TaskCommand) -> Result<(), String> {
    let target = match &command {
        TaskCommand::Show { task } | TaskCommand::Update { task, .. } => task.as_deref(),
    };
    let mut task = tool_task(config, target)?;
    if let TaskCommand::Update {
        title,
        description,
        description_file,
        fill_empty,
        ..
    } = command
    {
        let description = match description_file {
            Some(path) if path == std::path::Path::new("-") => {
                let mut text = String::new();
                std::io::stdin().read_to_string(&mut text).map_err(err)?;
                Some(text)
            }
            Some(path) => Some(std::fs::read_to_string(path).map_err(err)?),
            None => description,
        };
        if title.is_none() && description.is_none() {
            return Err("supply --title, --description or --description-file".into());
        }
        let untitled = task.title().is_none();
        task = update_context(task, title.as_deref(), description.as_deref(), fill_empty)?;
        if untitled && task.title().is_some() {
            name_work_after_title(config, &task, &mut |line| eprintln!("{line}"));
        }
    }
    println!(
        "{}",
        serde_json::json!({
            "id": task.name,
            "title": task.title(),
            "description": task.note.description(),
            "note": task.note.path,
            "done": task.done,
        })
    );
    Ok(())
}

/// Shared by task update and rename; never changes the identity or file path.
fn update_context(
    mut task: Task,
    title: Option<&str>,
    description: Option<&str>,
    fill_empty: bool,
) -> Result<Task, String> {
    if title.is_some_and(|s| s.trim().is_empty() || s.chars().any(char::is_control)) {
        return Err("title must be a non-empty single line".into());
    }
    if description.is_some_and(|s| s.trim().is_empty()) {
        return Err("description must not be empty".into());
    }
    let _lock = crate::vault::lock().map_err(err)?;
    let mut saved = false;
    for _ in 0..3 {
        task.note.reload().map_err(err)?;
        // Re-read under the lock, so another agent's first update wins.
        task = Task::from_note(task.note);
        if let Some(title) = title
            && (!fill_empty || task.title().is_none())
        {
            task.note
                .set("title", serde_yaml::Value::String(title.trim().into()));
        }
        if let Some(description) = description
            && (!fill_empty || task.note.description().is_empty())
        {
            task.note.set_description(description);
        }
        match task.note.save() {
            Ok(_) => {
                saved = true;
                break;
            }
            Err(NoteError::Changed(_)) => continue,
            Err(e) => return Err(err(e)),
        }
    }
    if !saved {
        return Err("task keeps changing, not written".into());
    }
    Ok(task)
}

/// A task started without a title runs in a space, tab and branch named
/// after its ID. Once it has a title they take it: the space its title, each
/// worktree's tab and branch a slug of it. A branch already on origin (so
/// maybe with an MR) or not named after the ID keeps its name; worktree
/// folders and the note never move — the running agent works in them.
fn name_work_after_title(config: &Config, task: &Task, say: &mut dyn FnMut(String)) {
    if herdr::available() {
        let renamed = herdr::list_workspaces().map_err(err).and_then(|spaces| {
            let identities = load(config)?;
            let space = crate::vault::task::task_space_in(task, &spaces, &identities)?;
            match space {
                Some(space) if !same_name(&space.label, &task.space_name()) => {
                    herdr::rename_workspace(&space.workspace_id, &task.space_name())
                        .map_err(err)?;
                    Ok(Some(task.space_name()))
                }
                _ => Ok(None),
            }
        });
        match renamed {
            Ok(Some(label)) => say(format!("space: {label}")),
            Ok(None) => {}
            Err(e) => say(format!("space: not renamed — {e}")),
        }
    }
    let Some(slug) = task.title().as_deref().and_then(name_from_text) else {
        say("branch: kept — the title has no Latin words to name it after".into());
        return;
    };
    let prefix = &config.workflow.branch_prefix;
    let tabs = if herdr::available() {
        herdr::list_tabs().unwrap_or_default()
    } else {
        Vec::new()
    };
    for path in task.note.strings("worktrees") {
        let path = PathBuf::from(path);
        let Some(dir) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else {
            continue;
        };
        let Ok(branch) = worktree::current_branch(&path) else {
            continue;
        };
        if !dir.starts_with(&task.name) || branch != format!("{prefix}{dir}") {
            continue;
        }
        if worktree::on_origin(&path, &branch) {
            say(format!("branch {branch}: kept — it is on origin"));
            continue;
        }
        let name = (1..)
            .map(|n| {
                if n == 1 {
                    slug.clone()
                } else {
                    format!("{slug}-{n}")
                }
            })
            .find(|name| !worktree::branch_exists(&path, &format!("{prefix}{name}")))
            .unwrap_or_else(|| slug.clone());
        let new = format!("{prefix}{name}");
        match worktree::rename_branch(&path, &branch, &new) {
            Ok(()) => say(format!("branch: {branch} → {new}")),
            Err(e) => {
                say(format!("branch {branch}: not renamed — {e}"));
                continue;
            }
        }
        for tab in tabs.iter().filter(|t| t.label == dir) {
            match herdr::rename_tab(&tab.tab_id, &name) {
                Ok(()) => say(format!("tab: {dir} → {name}")),
                Err(e) => say(format!("tab {dir}: not renamed — {e}")),
            }
        }
    }
}

pub(in crate::cli) fn cli_rename(
    config: &Config,
    args: Vec<String>,
    tab: Option<String>,
) -> Result<(), String> {
    let (target, title) = match args.as_slice() {
        [title] => (None, title.as_str()),
        [task, title] => (Some(task.as_str()), title.as_str()),
        _ => return Err("berbr rename [<task>] \"<new title>\"".into()),
    };
    let mut task = tool_task(config, target)?;
    // Resolve before updating: the existing space can still carry the old title.
    let spaces = if herdr::available() {
        herdr::list_workspaces().map_err(err)?
    } else {
        Vec::new()
    };
    let identities = load(config)?;
    let matched = crate::vault::task::task_space_in(&task, &spaces, &identities)?;
    task = update_context(task, Some(title), None, false)?;
    println!(
        "note: {} — title: {}",
        task.note.path.display(),
        task.space_name()
    );
    if let Some(space) = matched {
        herdr::rename_workspace(&space.workspace_id, &task.space_name()).map_err(err)?;
        println!("space: {} — {}", space.workspace_id, task.space_name());
    }
    if let Some(label) = tab {
        match herdr::caller_tab().map_err(err)? {
            Some(id) => {
                herdr::rename_tab(&id, &label).map_err(err)?;
                println!("tab: {id} — {label}");
            }
            None => println!("tab: skipped — cannot determine the calling terminal's tab"),
        }
    }
    Ok(())
}

fn tool_task(config: &Config, target: Option<&str>) -> Result<Task, String> {
    let mut tasks = load(config)?;
    if let Some(target) = target {
        let i = find_task(&tasks, target, true)?.ok_or_else(|| format!("no task {target}"))?;
        return Ok(tasks.swap_remove(i));
    }
    let mut table = table::build(config, tasks, false);
    let bound = std::env::current_dir()
        .ok()
        .and_then(|dir| worktree::worktree_root(&dir).ok())
        .map(|root| bound_context_task(&table, &root))
        .transpose()?
        .flatten();
    let i = bound
        .or_else(|| current_task(&table))
        .ok_or("no task here; pass its ID explicitly")?;
    Ok(table.tasks.swap_remove(i).task)
}

/// Explicit note bindings win over session/ticket/workspace inference for
/// context tools. Keep this separate from table ownership and close planning.
fn bound_context_task(table: &table::Table, root: &Path) -> Result<Option<usize>, String> {
    let canonical = |path: &Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let root = canonical(root);
    let references: Vec<_> = table
        .tasks
        .iter()
        .map(|row| {
            row.task
                .note
                .strings("worktrees")
                .into_iter()
                .chain(row.task.note.strings_at(&["bindings", "worktrees"]))
                .collect::<Vec<_>>()
        })
        .collect();
    // Absolute paths carry repository identity; a basename alone does not.
    for absolute in [true, false] {
        let mut matches = Vec::new();
        for (i, row) in table
            .tasks
            .iter()
            .enumerate()
            .filter(|(_, row)| !row.task.done)
        {
            let matches_root = references[i].iter().any(|reference| {
                let path = Path::new(reference);
                if absolute {
                    path.is_absolute() && canonical(path) == root
                } else {
                    !path.is_absolute()
                        && root.file_name().and_then(|n| n.to_str()) == Some(reference)
                }
            });
            if !matches_root {
                continue;
            }
            if !absolute
                && table
                    .trees
                    .iter()
                    .filter(|tree| tree.wt.path.file_name() == root.file_name())
                    .count()
                    > 1
            {
                return Err("ambiguous worktree name; save its full path in the task note or pass the task ID explicitly".into());
            }
            matches.push((i, row.task.name.as_str()));
        }
        match matches.as_slice() {
            [] => {}
            [(i, _)] => return Ok(Some(*i)),
            _ => {
                return Err(format!(
                    "worktree belongs to several tasks ({}); pass the task ID explicitly",
                    matches
                        .iter()
                        .map(|(_, name)| *name)
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }
    }
    Ok(None)
}

/// `berbr note "<text>"`: an agent's finding, appended to `## Agent notes` of the
/// task it works on (by its worktree, or by its herdr workspace).
pub(in crate::cli) fn cli_note(
    config: &Config,
    target: Option<String>,
    text: Vec<String>,
) -> Result<(), String> {
    let text = text.join(" ").trim().to_string();
    if text.is_empty() {
        return Err("berbr note \"<text>\"".to_string());
    }
    let mut task = tool_task(config, target.as_deref())?;

    let agent = std::env::var("HERDR_PANE_ID")
        .ok()
        .and_then(|pane| {
            herdr::list_agents()
                .ok()?
                .into_iter()
                .find(|a| a.pane_id == pane)?
                .name
        })
        .unwrap_or_else(|| "agent".to_string());
    let line = format!(
        "{} {agent}: {text}",
        chrono::Local::now().format("%Y-%m-%d")
    );

    let _lock = crate::vault::lock().map_err(err)?;
    for _ in 0..3 {
        // The pre-lock read may pair old text with a newer file timestamp.
        // Reload under the shared lock, just like task update does.
        task.note.reload().map_err(err)?;
        task.note.append_agent_note(&line);
        match task.note.save() {
            Ok(_) => {
                println!("written to \"{}\"", task.name);
                return Ok(());
            }
            Err(NoteError::Changed(_)) => continue,
            Err(e) => return Err(err(e)),
        }
    }
    Err(format!("\"{}\" keeps changing, not written", task.name))
}

#[cfg(test)]
mod context_tests {
    use super::*;
    use crate::table::fixtures::{ROOT, table, task_note, tree};

    #[test]
    fn absolute_binding_wins_over_basename_and_closed_tasks_are_ignored() {
        let table = table(
            vec![
                task_note(
                    "exact",
                    &format!("bindings:\n  worktrees: [{ROOT}/parser]"),
                    "2026-11-01",
                ),
                task_note("basename", "worktrees: [parser]", "2026-11-01"),
                task_note(
                    "closed",
                    &format!("done: true\nworktrees: [{ROOT}/parser]"),
                    "2026-11-01",
                ),
            ],
            vec![],
            vec![],
            vec![tree("parser", "parser")],
        );
        let i = bound_context_task(&table, &Path::new(ROOT).join("parser"))
            .unwrap()
            .unwrap();
        assert_eq!(table.tasks[i].task.name, "exact");
    }

    #[test]
    fn basename_binding_is_refused_when_two_repositories_have_the_same_tree_name() {
        let mut other = tree("parser", "parser");
        other.repo = "/other".into();
        other.wt.path = "/other/.herdr/worktrees/parser".into();
        let table = table(
            vec![task_note("owner", "worktrees: [parser]", "2026-11-01")],
            vec![],
            vec![],
            vec![tree("parser", "parser"), other],
        );
        assert!(
            bound_context_task(&table, &Path::new(ROOT).join("parser"))
                .unwrap_err()
                .contains("ambiguous worktree name")
        );
    }
}
