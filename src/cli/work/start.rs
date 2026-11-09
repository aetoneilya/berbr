//! `berbr start` and `berbr add`: new tasks and tickets with their worktree tabs.

use super::brief::brief_text;
use super::common::{
    Terminal, Ui, branch_name, choose_repo, err, fetch_issue, find_task, is_ticket_key, list,
    name_from_issue, resolve_task_table, work_name,
};
use super::launch::{Context, Open, Space, launch, open_added_work, open_work};
use crate::cli::{AddArgs, StartArgs, load};
use crate::client::jira::JiraIssue;
use crate::client::worktree;
use crate::config::Config;
use crate::vault::task::{Task, ticket_keys};
use serde_yaml::Value;

/// `berbr start <task name | KEY>`: a new task (note, herdr workspace, worktree tab
/// with an agent), or work on a ticket without a note.
pub(in crate::cli) fn cli_start(config: &Config, args: StartArgs) -> Result<(), String> {
    start(config, args, &mut Terminal)
}

pub(crate) fn start(config: &Config, args: StartArgs, ui: &mut dyn Ui) -> Result<(), String> {
    let _operation = crate::client::herdr::operation();
    start_with(config, load(config)?, args, ui)
}

fn start_with(
    config: &Config,
    tasks: Vec<Task>,
    mut args: StartArgs,
    ui: &mut dyn Ui,
) -> Result<(), String> {
    if let Some(name) = &args.name {
        worktree::validate_name(name).map_err(err)?;
    }
    args.description = args.description.filter(|d| !d.trim().is_empty());
    let target = match args
        .target
        .take()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
    {
        Some(target) => target,
        None if args.unnamed => String::new(),
        None if args.description.is_some() => {
            // a headless call of its own: works without an agent for the task too
            ui.say("generating a task name from the description…".into());
            let names = super::naming::from_description(
                &config.workflow,
                args.description.as_deref().unwrap(),
            )?;
            if args.name.is_none() {
                worktree::validate_name(&names.worktree).map_err(err)?;
                args.name = Some(names.worktree);
            }
            ui.say(format!("task name: {}", names.title));
            names.title
        }
        None => pick_start_target(config, &tasks, ui)?,
    };
    if target.trim().is_empty() && (args.no_agent || args.new_ticket) {
        return Err(
            "a task title is required without an agent or when creating a Jira ticket".into(),
        );
    }
    let existing = find_task(&tasks, &target, false)?;

    if is_ticket_key(&target) {
        if let Some(i) = existing {
            let task = &tasks[i];
            ui.say(format!(
                "{target} is already in task \"{}\" — adding it there",
                task.name
            ));
            let add_args = AddArgs {
                args: vec![task.name.clone(), target],
                in_tree: None,
                name: args.name,
                message: args.message,
                description: args.description,
                bg: args.bg,
            };
            return add_with(config, tasks, add_args, ui);
        }
        // work always gets a task: a note for shared context and agents' findings,
        // named after the ticket
        let issue = fetch_issue(config, &target)?;
        let title = note_title(&issue.fields.summary);
        if title.is_empty() {
            return Err(format!("{target} has no summary to name the task after"));
        }
        let mut keys = vec![target.clone()];
        keys.extend(args.tickets.into_iter().filter(|k| k != &target));
        let args = StartArgs {
            target: Some(title),
            tickets: keys,
            ..args
        };
        return start_with(config, tasks, args, ui);
    }

    if let Some(i) = existing {
        return Err(format!(
            "task \"{0}\" already exists: berbr select \"{0}\" to get back to it, berbr add \"{0}\" <KEY> to add a ticket",
            tasks[i].name
        ));
    }
    let StartArgs {
        tickets,
        tags,
        message,
        description,
        name,
        branch,
        bg,
        new_ticket,
        no_worktree,
        no_agent,
        ..
    } = args;
    let similar = similar_tasks(&tasks, &target, &tags);
    if !similar.is_empty() && ui.can_ask() {
        ui.say(format!("similar tasks: {}", similar.join(", ")));
        if !ui.confirm("Create a new one anyway?")? {
            return Err("not created: berbr select / berbr add if it is one of them".to_string());
        }
    }

    let mut keys: Vec<String> = tickets.iter().flat_map(|t| ticket_keys(t)).collect();
    let tree_name = name;
    if new_ticket {
        let key = create_ticket(config, &target)?;
        ui.say(format!("{key} created in Jira"));
        keys.insert(0, key);
    }

    let mut frontmatter = Vec::new();
    if !keys.is_empty() {
        frontmatter.push(("tickets", list(&keys)));
    }
    if !tags.is_empty() {
        frontmatter.push(("tags", list(&tags)));
    }
    frontmatter.push(("done", Value::Bool(false)));
    let body = [message.as_deref(), description.as_deref()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("\n\n");
    let body = if body.is_empty() {
        body
    } else {
        format!("{body}\n")
    };
    let note = {
        let _lock = crate::vault::lock().map_err(err)?;
        crate::vault::identity::create(config, &target, &frontmatter, &body)?
    };
    let path = note.path.clone();
    let task = Task::from_note(note);
    ui.say(format!("note: {}", path.display()));
    if no_worktree && no_agent {
        return Ok(());
    }

    let issue = keys.first().map(|k| fetch_issue(config, k)).transpose()?;
    let main = choose_repo(config, ui)?;
    if no_worktree {
        // The agent in the main checkout, its brief in berbr's cache
        let label = work_name(&task, tree_name, issue.as_ref());
        let branch = worktree::current_branch(&main).unwrap_or_default();
        let text = brief_text(
            &config.name,
            &main,
            &branch,
            Some(&task),
            issue.as_ref(),
            &keys,
            description.as_deref().or(message.as_deref()),
        );
        let brief = crate::config::cache_dir()
            .join("briefs")
            .join(format!("{}.md", task.name.replace('/', "-")));
        if let Some(dir) = brief.parent() {
            std::fs::create_dir_all(dir).map_err(err)?;
        }
        std::fs::write(&brief, text).map_err(err)?;
        return launch(
            config.workflow.agent,
            Space::Task(&task),
            &main,
            &label,
            Context {
                note: Some(&task.note.path),
                brief: &brief,
                description: description.as_deref(),
                display_name: Some(&task.space_name()),
            },
            !bg,
            ui,
        );
    }

    let desired = work_name(&task, tree_name, issue.as_ref());
    let wt_name = worktree::reserve_name(
        &main,
        &config.workflow.worktree_dir,
        &desired,
        &task.name,
        &config.workflow.branch_prefix,
        keys.first().map(String::as_str),
    )
    .map_err(err)?;
    if desired != wt_name {
        ui.say(format!("{desired} is reserved; using {wt_name}"));
    }
    let branch = branch.unwrap_or_else(|| branch_name(config, keys.first(), &wt_name));
    let tree = worktree::worktree_dir(&main, &config.workflow.worktree_dir, &wt_name);
    let brief = brief_text(
        &config.name,
        &tree,
        &branch,
        Some(&task),
        issue.as_ref(),
        &keys,
        description.as_deref().or(message.as_deref()),
    );
    let open = Open {
        directory: config.workflow.worktree_dir.clone(),
        kind: config.workflow.agent,
        agent: !no_agent,
        focus: !bg,
        description,
    };
    open_work(&task, &main, &wt_name, &branch, brief, open, ui)
}

/// A ticket for task `title` in the project's Jira, assigned to me.
fn create_ticket(config: &Config, title: &str) -> Result<String, String> {
    let client = config.jira_client()?;
    let jira = config.jira.as_ref().ok_or("no Jira")?;
    let project = jira
        .project
        .as_deref()
        .ok_or("no [project.jira] project in the config to create the ticket in")?;
    client
        .create_issue(project, &jira.issue_type, title)
        .map_err(err)
}

/// `berbr add [<task>] [<KEY>]`: a new agent tab in the task's herdr workspace,
/// in a new worktree or `--in` one the task has. KEY is optional with --name/--in.
pub(in crate::cli) fn cli_add(config: &Config, args: AddArgs) -> Result<(), String> {
    add(config, args, &mut Terminal).map_err(|e| e.lines().collect::<Vec<_>>().join(" "))
}

pub(crate) fn add(config: &Config, args: AddArgs, ui: &mut dyn Ui) -> Result<(), String> {
    let _operation = crate::client::herdr::operation();
    add_with(config, load(config)?, args, ui)
}

fn add_with(
    config: &Config,
    tasks: Vec<Task>,
    args: AddArgs,
    ui: &mut dyn Ui,
) -> Result<(), String> {
    let AddArgs {
        args,
        in_tree,
        name,
        message,
        description,
        bg,
    } = args;
    let (task_arg, key) = match args.as_slice() {
        [] if in_tree.is_some() || name.is_some() => (None, None),
        [task] if (in_tree.is_some() || name.is_some()) && !is_ticket_key(task) => {
            (Some(task.clone()), None)
        }
        [key] => (None, Some(key.clone())),
        [task, key] => (Some(task.clone()), Some(key.clone())),
        _ => {
            return Err(
                "berbr add [<task>] [<KEY>] --name <name>, or berbr add [<task>] --in <tree>"
                    .to_string(),
            );
        }
    };
    if let Some(key) = &key
        && !is_ticket_key(key)
    {
        return Err(format!(
            "\"{key}\" is not a ticket key: berbr add [<task>] [<KEY>] --name <name>, or use --in <tree>"
        ));
    }
    let (mut table, row) = resolve_task_table(config, tasks, task_arg.as_deref(), ui)?;
    let owned: Vec<_> = table.tasks[row]
        .trees
        .iter()
        .map(|&i| (table.tree(i).repo.clone(), table.tree(i).wt.clone()))
        .collect();
    let mut task = table.tasks.swap_remove(row).task;
    if let Some(key) = &key
        && task.tickets.contains(key)
        && name.is_none()
        && in_tree.is_none()
    {
        return Err(format!(
            "{key} is already in task \"{}\"; pass --name <name> for a new worktree or --in <tree> for an existing one",
            task.name
        ));
    }
    let issue = key
        .as_deref()
        .filter(|_| config.jira.is_some())
        .map(|key| fetch_issue(config, key))
        .transpose()?;
    let main = match &in_tree {
        Some(existing) => {
            let matches: Vec<_> = owned
                .iter()
                .filter(|(_, wt)| {
                    wt.name() == *existing || wt.path.to_str() == Some(existing.as_str())
                })
                .collect();
            match matches.as_slice() {
                [(repo, _)] => repo.clone(),
                [] => {
                    return Err(format!(
                        "{} has no worktree {existing}; --in only accepts this task's worktrees",
                        task.name
                    ));
                }
                _ => {
                    return Err(format!(
                        "worktree {existing} is ambiguous; use its full path"
                    ));
                }
            }
        }
        None => super::common::choose_discovered_repo(&table.mains, ui)?,
    };
    let existing = in_tree.is_some();
    let (tree, branch, wt_name) = match in_tree {
        Some(existing) => {
            let dir = if std::path::Path::new(&existing).is_absolute() {
                existing.clone().into()
            } else {
                worktree::existing_worktree(&main, &config.workflow.worktree_dir, &existing)
                    .map_err(err)?
                    .ok_or_else(|| format!("no worktree {existing}"))?
            };
            if !dir.exists() {
                return Err(format!("no worktree {}", dir.display()));
            }
            let branch = worktree::current_branch(&dir).map_err(err)?;
            let name = dir
                .file_name()
                .ok_or("worktree has no name")?
                .to_string_lossy()
                .into_owned();
            (dir, branch, name)
        }
        None => {
            let wt_name = name
                .or_else(|| issue.as_ref().map(name_from_issue))
                .ok_or("pass --name <name> when Jira is not configured")?;
            worktree::validate_name(&wt_name).map_err(err)?;
            let branch = branch_name(config, key.as_ref(), &wt_name);
            let tree = worktree::worktree_dir(&main, &config.workflow.worktree_dir, &wt_name);
            if tree.exists() {
                return Err(format!(
                    "worktree {} already exists; pass --in {wt_name} or another --name",
                    tree.display()
                ));
            }
            (tree, branch, wt_name)
        }
    };
    if !crate::client::herdr::available() {
        return Err("berbr add needs Herdr to open the worktree tab and agent".into());
    }
    if !existing {
        worktree::reserve_exact_name(&main, &config.workflow.worktree_dir, &wt_name, &task.name)
            .map_err(err)?;
        worktree::create_new_worktree(&main, &config.workflow.worktree_dir, &wt_name, &branch)
            .map_err(err)?;
    }
    let bind_tree = !existing && key.is_none();
    if bind_tree || key.as_ref().is_some_and(|key| !task.tickets.contains(key)) {
        let record_error = |e: String| {
            format!(
                "worktree {} exists; could not update task note: {e}",
                tree.display()
            )
        };
        let _lock = crate::vault::lock().map_err(err).map_err(&record_error)?;
        task.note.reload().map_err(err).map_err(&record_error)?;
        task = Task::from_note(task.note);
        if let Some(key) = &key
            && !task.tickets.contains(key)
        {
            task.tickets.push(key.clone());
            task.note.set("tickets", list(&task.tickets));
        }
        if bind_tree {
            let mut trees = task.note.strings("worktrees");
            let path = tree.to_string_lossy().into_owned();
            if !trees.contains(&path) {
                trees.push(path);
            }
            task.note.set("worktrees", list(&trees));
        }
        task.note.save().map_err(err).map_err(record_error)?;
    }
    let brief = brief_text(
        &config.name,
        &tree,
        &branch,
        Some(&task),
        issue.as_ref(),
        &task.tickets,
        description.as_deref().or(message.as_deref()),
    );
    let open = Open {
        directory: config.workflow.worktree_dir.clone(),
        kind: config.workflow.agent,
        agent: true,
        focus: !bg && ui.can_ask(),
        description,
    };
    open_added_work(&task, &tree, &wt_name, brief, open, ui).map_err(|e| {
        format!(
            "worktree {} exists; could not launch agent: {e}",
            tree.display()
        )
    })
}

/// My open Jira tickets that belong to no task, plus "new task…".
fn pick_start_target(config: &Config, tasks: &[Task], ui: &mut dyn Ui) -> Result<String, String> {
    let client = config.jira_client()?;
    let issues = client
        .search_issues_brief(
            "assignee = currentUser() AND statusCategory != Done ORDER BY updated DESC",
            50,
        )
        .map_err(err)?;
    let free: Vec<&JiraIssue> = issues
        .iter()
        .filter(|i| !tasks.iter().any(|t| t.tickets.contains(&i.key)))
        .collect();
    let mut items = vec!["+ new task…".to_string()];
    items.extend(
        free.iter()
            .map(|i| format!("{}  {}", i.key, i.fields.summary)),
    );
    match ui.pick("Start", &items)? {
        0 => ui.input("Task name"),
        i => Ok(free[i - 1].key.clone()),
    }
}

fn similar_tasks(tasks: &[Task], name: &str, tags: &[String]) -> Vec<String> {
    let words: Vec<String> = name
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 3)
        .map(str::to_string)
        .collect();
    tasks
        .iter()
        .filter(|t| !t.done)
        .filter(|t| {
            let title = t.display_name().to_lowercase();
            t.tags.iter().any(|tag| tags.contains(tag))
                || words.iter().any(|w| title.contains(w.as_str()))
        })
        .map(|t| t.display_name())
        .collect()
}

/// A ticket summary as a note file name: without the characters Obsidian and
/// file systems refuse, one line, of reasonable length.
pub(super) fn note_title(summary: &str) -> String {
    let cleaned: String = summary
        .chars()
        .map(|c| {
            if "*\"\\/<>:|?#^[]".contains(c) || c.is_control() {
                ' '
            } else {
                c
            }
        })
        .collect();
    let title = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    title
        .chars()
        .take(120)
        .collect::<String>()
        .trim_end_matches(['.', ' '])
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::note_title;

    #[test]
    fn ticket_summary_as_note_title() {
        assert_eq!(
            note_title("javadoc-и ко всем точкам расширения"),
            "javadoc-и ко всем точкам расширения"
        );
        assert_eq!(
            note_title("s2s: читать [table]/path?  "),
            "s2s читать table path"
        );
    }
}
