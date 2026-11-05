//! Helpers shared by the work commands: errors, names, repo, task and worktree
//! lookup, pickers.

use crate::client::jira::JiraIssue;
use crate::client::worktree;
use crate::config::Config;
use crate::table::{self, Table, TaskRow};
use crate::vault::task::{Task, ticket_keys};
use dialoguer::theme::ColorfulTheme;
use dialoguer::{Confirm, FuzzySelect, Input};
use serde_yaml::Value;
use std::io::IsTerminal;
use std::path::PathBuf;

/// Where a command reports and asks: the terminal, or a TUI dialog.
pub(crate) trait Ui {
    fn say(&mut self, line: String);
    /// The index of the chosen item, or why there is none.
    fn pick(&mut self, prompt: &str, items: &[String]) -> Result<usize, String>;
    fn confirm(&mut self, prompt: &str) -> Result<bool, String>;
    fn input(&mut self, prompt: &str) -> Result<String, String>;
    /// Whether questions reach anyone: without that, what a command can do
    /// without asking it does unasked.
    fn can_ask(&self) -> bool {
        true
    }
}

/// The command line: dialoguer prompts, stdout.
pub(crate) struct Terminal;

impl Ui for Terminal {
    fn say(&mut self, line: String) {
        println!("{line}");
    }

    fn pick(&mut self, prompt: &str, items: &[String]) -> Result<usize, String> {
        fuzzy(prompt, items)
    }

    fn confirm(&mut self, prompt: &str) -> Result<bool, String> {
        confirm(prompt)
    }

    fn input(&mut self, prompt: &str) -> Result<String, String> {
        Input::with_theme(&ColorfulTheme::default())
            .with_prompt(prompt)
            .interact_text()
            .map_err(err)
    }

    fn can_ask(&self) -> bool {
        std::io::stdin().is_terminal()
    }
}

pub(super) fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

pub(crate) fn is_ticket_key(text: &str) -> bool {
    ticket_keys(text).first().is_some_and(|k| k == text)
}

pub(super) use crate::vault::task::same_name;

pub(super) fn work_name(
    task: &Task,
    explicit: Option<String>,
    issue: Option<&JiraIssue>,
) -> String {
    explicit
        .or_else(|| issue.map(name_from_issue))
        .or_else(|| task.title().as_deref().and_then(name_from_text))
        .unwrap_or_else(|| task.name.clone())
}

pub(super) fn fetch_issue(config: &Config, key: &str) -> Result<JiraIssue, String> {
    config
        .jira_client()?
        .get_issue_by_key(key)
        .map_err(|e| format!("{key}: {e}"))
}

pub(super) fn name_from_issue(issue: &JiraIssue) -> String {
    name_from_text(&issue.fields.summary).unwrap_or_else(|| issue.key.to_lowercase())
}

/// The first four latin words, slugified.
pub(super) fn name_from_text(text: &str) -> Option<String> {
    let slug = slugify(text);
    let words: Vec<&str> = slug.split('-').filter(|w| !w.is_empty()).take(4).collect();
    (!words.is_empty()).then(|| words.join("-"))
}

fn slugify(text: &str) -> String {
    let mut slug = String::new();
    let mut prev_dash = false;

    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }

    slug.trim_matches('-').to_string()
}

pub(super) fn branch_name(config: &Config, key: Option<&String>, name: &str) -> String {
    let prefix = &config.workflow.branch_prefix;
    match key {
        Some(key) => format!("{prefix}{key}-{name}"),
        None => format!("{prefix}{name}"),
    }
}

/// The main checkout new work goes into: the only configured repo, the one the
/// cwd is in, else the one picked; the first when nobody can be asked.
pub(super) fn choose_repo(config: &Config, ui: &mut dyn Ui) -> Result<PathBuf, String> {
    let mains: Vec<PathBuf> = config
        .repos
        .iter()
        .filter_map(|r| worktree::main_checkout(&r.path).ok())
        .collect();
    let here = std::env::current_dir()
        .ok()
        .and_then(|d| worktree::main_checkout(&d).ok());
    match mains.len() {
        0 => return Err("no [[repo]] in ~/.config/berbr/config.toml".to_string()),
        1 => return Ok(mains[0].clone()),
        _ => {}
    }
    if let Some(main) = here.filter(|m| mains.contains(m)) {
        return Ok(main);
    }
    if !ui.can_ask() {
        return Ok(mains[0].clone());
    }
    let names: Vec<String> = mains.iter().map(|m| m.display().to_string()).collect();
    Ok(mains[ui.pick("Repository", &names)?].clone())
}

/// The main checkout that has worktree `name`, if any repo has it.
pub(super) fn repo_with_worktree(config: &Config, name: &str) -> Option<PathBuf> {
    config
        .repos
        .iter()
        .filter_map(|r| worktree::main_checkout(&r.path).ok())
        .find(|main| worktree::worktree_dir(main, name).exists())
}

pub(super) fn list(items: &[String]) -> Value {
    Value::Sequence(items.iter().cloned().map(Value::String).collect())
}

fn fuzzy(prompt: &str, items: &[String]) -> Result<usize, String> {
    if !std::io::stdin().is_terminal() {
        return Err(format!(
            "{prompt}: needs a choice, but there is no terminal — pass an argument"
        ));
    }
    FuzzySelect::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .items(items)
        .default(0)
        .interact_opt()
        .map_err(err)?
        .ok_or_else(|| "cancelled".to_string())
}

pub(super) fn confirm(prompt: &str) -> Result<bool, String> {
    Confirm::with_theme(&ColorfulTheme::default())
        .with_prompt(prompt)
        .default(false)
        .interact()
        .map_err(err)
}

/// The open task of the current worktree, or of the current herdr workspace.
pub(super) fn current_task(table: &Table) -> Option<usize> {
    let open = |r: &TaskRow| !r.task.done;
    if let Some(root) = std::env::current_dir()
        .ok()
        .and_then(|d| worktree::worktree_root(&d).ok())
    {
        let by_tree = table
            .tasks
            .iter()
            .position(|r| open(r) && r.trees.iter().any(|&t| table.tree(t).wt.path == root));
        if by_tree.is_some() {
            return by_tree;
        }
    }
    let space = std::env::var("HERDR_WORKSPACE_ID").ok()?;
    table
        .tasks
        .iter()
        .position(|r| open(r) && r.space.as_deref() == Some(space.as_str()))
}

/// A task by ticket key, or by name; open tasks win over done ones. With
/// `partial`, otherwise the only open task whose name contains `target`.
/// Index into `tasks`.
pub(super) fn find_task<'a, I>(
    tasks: I,
    target: &str,
    partial: bool,
) -> Result<Option<usize>, String>
where
    I: IntoIterator<Item = &'a Task>,
    I::IntoIter: Clone,
{
    let tasks = tasks.into_iter();
    // An immutable ID must win over another task's editable title.
    if let Some(i) = tasks
        .clone()
        .position(|t| !t.done && same_name(&t.name, target))
        .or_else(|| tasks.clone().position(|t| same_name(&t.name, target)))
    {
        return Ok(Some(i));
    }
    let names = |t: &Task| {
        if is_ticket_key(target) {
            t.tickets.iter().any(|k| k == target)
        } else {
            same_name(&t.name, target) || t.title().is_some_and(|title| same_name(&title, target))
        }
    };
    let mut exact: Vec<usize> = tasks
        .clone()
        .enumerate()
        .filter(|(_, t)| !t.done && names(t))
        .map(|(i, _)| i)
        .collect();
    if exact.is_empty() {
        exact = tasks
            .clone()
            .enumerate()
            .filter(|(_, t)| names(t))
            .map(|(i, _)| i)
            .collect();
    }
    match exact.as_slice() {
        [i] => return Ok(Some(*i)),
        [] if !partial => return Ok(None),
        [] => {}
        _ => return Err(format!("\"{target}\" matches several tasks; use its ID")),
    }
    let needle = target.to_lowercase();
    let matches: Vec<usize> = tasks
        .enumerate()
        .filter(|(_, t)| !t.done && t.display_name().to_lowercase().contains(&needle))
        .map(|(i, _)| i)
        .collect();
    match matches.as_slice() {
        [] => Ok(None),
        [i] => Ok(Some(*i)),
        _ => Err(format!(
            "\"{target}\" matches several tasks, be more specific"
        )),
    }
}

/// A task by `find_task`, otherwise the current one, otherwise a picker.
pub(super) fn resolve_task(
    config: &Config,
    mut tasks: Vec<Task>,
    arg: Option<&str>,
    ui: &mut dyn Ui,
) -> Result<Task, String> {
    if let Some(arg) = arg {
        return match find_task(&tasks, arg, true)? {
            Some(i) => Ok(tasks.swap_remove(i)),
            None => Err(format!(
                "no task \"{arg}\": berbr start \"{arg}\" creates it"
            )),
        };
    }
    let mut table = table::build(config, tasks, false);
    if let Some(i) = current_task(&table) {
        return Ok(table.tasks.swap_remove(i).task);
    }
    if !ui.can_ask() {
        return Err("no task here; pass its ID explicitly: berbr add <task> <KEY>".into());
    }
    let open: Vec<usize> = (0..table.tasks.len())
        .filter(|&i| !table.tasks[i].task.done)
        .collect();
    let items: Vec<String> = open
        .iter()
        .map(|&i| table.tasks[i].task.display_name())
        .collect();
    let chosen = open[ui.pick("Task", &items)?];
    Ok(table.tasks.swap_remove(chosen).task)
}

/// Every worktree of the configured repos; `online` adds MRs, changes and
/// unpushed counts.
pub(super) fn tree_table(config: &Config, online: bool) -> Result<Table, String> {
    if config.repos.is_empty() {
        return Err("no [[repo]] in ~/.config/berbr/config.toml".to_string());
    }
    // tasks don't decide which worktree is which, and are not needed here
    Ok(table::build(config, Vec::new(), online))
}

/// A worktree by name or branch, otherwise the one containing the cwd, otherwise
/// a picker. Index into `table.trees`.
pub(super) fn resolve_tree(
    table: &Table,
    name: Option<String>,
    ui: &mut dyn Ui,
) -> Result<usize, String> {
    let trees = &table.trees;

    if let Some(name) = name {
        return trees
            .iter()
            .position(|t| t.wt.name() == name || t.wt.branch.as_deref() == Some(name.as_str()))
            .ok_or_else(|| format!("No worktree named {name}"));
    }

    let root = std::env::current_dir()
        .ok()
        .and_then(|d| worktree::worktree_root(&d).ok());
    if let Some(i) = root.and_then(|root| trees.iter().position(|t| t.wt.path == root)) {
        return Ok(i);
    }

    if trees.is_empty() {
        return Err("No linked worktrees".to_string());
    }
    let names: Vec<String> = trees.iter().map(|t| t.wt.name()).collect();
    ui.pick("Worktree", &names)
}

#[cfg(test)]
mod naming_tests {
    use super::*;
    use crate::table::fixtures::task_note;

    #[test]
    fn work_names_use_title_then_identity_and_respect_explicit_and_ticket_names() {
        let mut task = task_note(
            "demo-0",
            "id: demo-0\ntitle: 'Parser: исправить repeated calls with cache'",
            "2026-11-01",
        );
        assert_eq!(work_name(&task, None, None), "parser-repeated-calls-with");
        let issue: JiraIssue = serde_json::from_value(serde_json::json!({
            "key":"DEMO-42", "fields":{"summary":"Ticket summary", "created":"2026-11-01T00:00:00Z"}
        }))
        .unwrap();
        assert_eq!(work_name(&task, None, Some(&issue)), "ticket-summary");
        assert_eq!(
            work_name(&task, Some("chosen".into()), Some(&issue)),
            "chosen"
        );
        task.note
            .set("title", Value::String("Только кириллица".into()));
        assert_eq!(work_name(&task, None, None), "demo-0");
        assert_eq!(work_name(&task, Some("chosen".into()), None), "chosen");
        let issue: JiraIssue = serde_json::from_value(serde_json::json!({
            "key":"DEMO-42", "fields":{"summary":"Только кириллица", "created":"2026-11-01T00:00:00Z"}
        })).unwrap();
        assert_eq!(work_name(&task, None, Some(&issue)), "demo-42");
        let legacy = task_note("Legacy title", "", "2026-11-01");
        assert_eq!(work_name(&legacy, None, None), "legacy-title");
    }
}
