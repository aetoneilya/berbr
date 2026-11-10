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

pub(in crate::cli) fn cli_artifacts(
    config: &Config,
    target: Option<&str>,
    list: bool,
    directory: Option<&Path>,
) -> Result<(), String> {
    // Polling file lists needs identities/bindings, not every task's complete
    // Agent notes and session history.
    let (tasks, skipped) = crate::vault::task::load_previews(config);
    for error in skipped {
        eprintln!("skipped {error}");
    }
    let task = resolve_tool_task(config, target, tasks)?;
    let root = crate::artifacts::ensure(&config.name, &task.name)?;
    if list {
        let directory = directory.unwrap_or_else(|| Path::new(""));
        let entries = crate::artifacts::list(&root, directory)?;
        println!(
            "{}",
            serde_json::json!({"project":config.name, "task":task.name, "path":root, "directory":directory, "entries":entries})
        );
    } else {
        println!("{}", root.display());
    }
    Ok(())
}

pub(in crate::cli) fn cli_task(config: &Config, command: TaskCommand) -> Result<(), String> {
    if let TaskCommand::List { json, all } = command {
        let (mut tasks, skipped) = crate::vault::task::load(config)?;
        tasks.retain(|task| all || !task.done);
        tasks.sort_by(|a, b| a.name.cmp(&b.name));
        let warnings: Vec<_> = skipped
            .into_iter()
            .map(|e| format!("skipped {e}"))
            .collect();
        if json {
            println!(
                "{}",
                serde_json::json!({
                    "version": 1, "project": config.name,
                    "tasks": tasks.iter().map(|t| serde_json::json!({
                        "id": t.name, "title": t.title(), "note": t.note.path,
                        "done": t.done, "tickets": t.tickets, "tags": t.tags,
                    })).collect::<Vec<_>>(), "warnings": warnings,
                })
            );
        } else {
            for task in tasks {
                println!(
                    "{}{}",
                    task.display_name(),
                    if task.done { " [done]" } else { "" }
                );
            }
            for warning in warnings {
                eprintln!("{warning}");
            }
        }
        return Ok(());
    }
    let target = match &command {
        TaskCommand::Show { task } | TaskCommand::Update { task, .. } => task.as_deref(),
        TaskCommand::List { .. } => unreachable!(),
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
    // A no-op needs only read access, including in an existing agent whose
    // sandbox does not grant the shared note directory or write lock.
    task.note.reload().map_err(err)?;
    task = Task::from_note(task.note);
    if !context_changes(&task, title, description, fill_empty) {
        return Ok(task);
    }
    let _lock = crate::vault::lock().map_err(err)?;
    let mut saved = false;
    for _ in 0..3 {
        task.note.reload().map_err(err)?;
        // Re-read under the lock, so another agent's first update wins.
        task = Task::from_note(task.note);
        if !context_changes(&task, title, description, fill_empty) {
            return Ok(task);
        }
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

fn context_changes(
    task: &Task,
    title: Option<&str>,
    description: Option<&str>,
    fill_empty: bool,
) -> bool {
    let current_title = task.title();
    title.is_some_and(|title| {
        (!fill_empty || current_title.is_none()) && current_title.as_deref() != Some(title.trim())
    }) || description.is_some_and(|description| {
        let current = task.note.description();
        (!fill_empty || current.is_empty()) && current != description.trim()
    })
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
    resolve_tool_task(config, target, load(config)?)
}

fn resolve_tool_task(
    config: &Config,
    target: Option<&str>,
    mut tasks: Vec<Task>,
) -> Result<Task, String> {
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
pub(super) fn bound_context_task(
    table: &table::Table,
    root: &Path,
) -> Result<Option<usize>, String> {
    let tasks: Vec<_> = table.tasks.iter().map(|r| &r.task).collect();
    let roots: Vec<_> = table.trees.iter().map(|t| t.wt.path.clone()).collect();
    let (matches, ambiguous_name) = super::context::bound_tasks(&tasks, root, &roots);
    if ambiguous_name {
        return Err("ambiguous worktree name; save its full path in the task note or pass the task ID explicitly".into());
    }
    match matches.as_slice() {
        [] => Ok(None),
        [i] => Ok(Some(*i)),
        _ => Err(format!(
            "worktree belongs to several tasks ({}); pass the task ID explicitly",
            matches
                .iter()
                .map(|&i| tasks[i].name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// One active owner for launch actions that inherit a task's space and artifacts.
/// An explicit worktree binding wins over inferred ticket/session associations.
pub(super) fn worktree_owner(
    table: &table::Table,
    tree: usize,
) -> Result<Option<&table::TaskRow>, String> {
    if let Some(index) = bound_context_task(table, &table.tree(tree).wt.path)? {
        return Ok(Some(&table.tasks[index]));
    }
    let owners: Vec<_> = table
        .tasks
        .iter()
        .filter(|row| !row.task.done && row.trees.contains(&tree))
        .collect();
    match owners.as_slice() {
        [] => Ok(None),
        [row] => Ok(Some(row)),
        _ => Err("worktree belongs to several tasks; cannot choose its shared artifacts".into()),
    }
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
