//! Task context tools and findings appended to task notes.

use super::common::{current_task, err, find_task};
use crate::cli::{TaskCommand, load};
use crate::client::herdr;
use crate::config::Config;
use crate::table;
use crate::vault::note::NoteError;
use crate::vault::task::Task;
use std::io::Read;

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
        task = update_context(task, title.as_deref(), description.as_deref(), fill_empty)?;
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
    let i = current_task(&table).ok_or("no task here; pass its ID explicitly")?;
    Ok(table.tasks.swap_remove(i).task)
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
        task.note.append_agent_note(&line);
        match task.note.save() {
            Ok(_) => {
                println!("written to \"{}\"", task.name);
                return Ok(());
            }
            Err(NoteError::Changed(_)) => task.note.reload().map_err(err)?,
            Err(e) => return Err(err(e)),
        }
    }
    Err(format!("\"{}\" keeps changing, not written", task.name))
}
