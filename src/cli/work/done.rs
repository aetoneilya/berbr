//! `berbr done` and `berbr attach`: closing a task or worktrees, attaching a ticket.

use super::common::{
    Terminal, Ui, confirm, err, find_task, is_ticket_key, list, resolve_task, resolve_tree,
};
use super::launch::{close_herdr_views, close_herdr_views_outside};
use crate::cli::DoneArgs;
use crate::cli::load;
use crate::client::gitlab::{MergeRequest, MrState};
use crate::client::herdr;
use crate::client::jira::{JiraClient, Transition};
use crate::client::worktree;
use crate::config::Config;
use crate::table::{self, Fresh, Table, Tree, branch_finished};
use crate::vault::note::Note;
use crate::vault::task::Task;
use serde_yaml::Value;

/// Positional targets are tasks only. Checkout removal has an explicit target
/// kind, so an editable task title or ticket alias can never redirect it.
pub(in crate::cli) fn cli_done(config: &Config, args: DoneArgs) -> Result<(), String> {
    if let Some(name) = args.name.as_deref() {
        let tasks = load(config)?;
        let i = find_task(&tasks, name, false)?.ok_or_else(|| {
            format!(
                "no task \"{name}\"; to remove a worktree, use berbr done --tree <name-or-path>"
            )
        })?;
        if tasks[i].done {
            return Err(format!("task \"{name}\" is already done"));
        }
        let plan = plan_task_with(config, tasks, name, args.force, args.dry_run)?;
        if args.dry_run {
            return plan.report(args.force).print(args.json);
        }
        if !plan.blockers.is_empty() {
            return Err(not_finished(plan.name(), &plan.blockers));
        }
        println!("Closing task \"{}\":", plan.name());
        for step in plan.steps() {
            println!("  {step}");
        }
        if !args.yes && !confirm(plan.question())? {
            return Ok(());
        }
        return finish_task(config, plan, args.force, &mut print);
    }
    done_trees(config, args)
}

fn print(line: String) {
    println!("{line}");
}

/// Serializable projection of the same plans executed by `done`. Candidate
/// removals and steps are conditional on the whole plan having no blockers.
#[derive(Default, serde::Serialize)]
struct DonePlan {
    kind: &'static str,
    name: Option<String>,
    blockers: Vec<String>,
    remove: Vec<Removal>,
    dirty: Vec<DirtyTree>,
    keep: Vec<Kept>,
    close_tickets: Vec<String>,
    leave_tickets: Vec<String>,
    note_done: bool,
    close_space: bool,
    steps: Vec<String>,
    #[serde(skip)]
    trees: Vec<usize>,
}

#[derive(serde::Serialize)]
struct DirtyTree {
    tree: String,
    path: std::path::PathBuf,
    uncommitted: usize,
}

#[derive(serde::Serialize)]
struct Removal {
    tree: String,
    path: std::path::PathBuf,
    // Unknown inspection results remain null, including with --force.
    uncommitted: Option<usize>,
    unpushed: Option<usize>,
    merged: bool,
}

#[derive(serde::Serialize)]
struct Kept {
    tree: String,
    reason: &'static str,
}

impl DonePlan {
    fn print(&self, json: bool) -> Result<(), String> {
        if json {
            println!("{}", serde_json::to_string(self).map_err(err)?);
        } else {
            for blocker in &self.blockers {
                println!("blocked: {blocker}");
            }
            for step in &self.steps {
                println!("  {step}");
            }
            for kept in &self.keep {
                println!("keep {}: {}", kept.tree, kept.reason);
            }
        }
        Ok(())
    }

    fn add_tree(&mut self, table: &Table, t: usize, blocked: bool, discard: bool) {
        let tree = table.tree(t);
        if blocked {
            let reason = if !discard && tree.changes != Some(0) {
                "uncommitted changes"
            } else if mr_of(table, tree).is_some_and(|m| m.state() == MrState::Opened) {
                "open MR"
            } else if tree.unpushed != Some(0) {
                "unpushed commits"
            } else {
                "not merged"
            };
            self.keep.push(Kept {
                tree: tree.wt.name(),
                reason,
            });
        } else {
            self.trees.push(t);
            self.remove.push(Removal {
                tree: tree.wt.name(),
                path: tree.wt.path.clone(),
                uncommitted: tree.changes,
                unpushed: tree.unpushed,
                merged: tree.finished
                    || mr_of(table, tree).is_some_and(|m| m.state() == MrState::Merged),
            });
        }
    }
}

fn plan_sweep(table: &Table, force: bool) -> Result<DonePlan, String> {
    let mut plan = DonePlan {
        kind: "trees",
        ..Default::default()
    };
    plan.blockers.extend(tree_data_blocker(table, force));
    for (t, tree) in table.trees.iter().enumerate() {
        if !tree.finished {
            // A sweep skips unfinished trees, including failed inspections,
            // without blocking removal of the finished ones.
            if tree.changes != Some(0) || !held(table, tree).is_empty() {
                plan.add_tree(table, t, true, false);
            } else {
                plan.keep.push(Kept {
                    tree: tree.wt.name(),
                    reason: "not merged",
                });
            }
            continue;
        }
        let blockers = tree_blockers(table, tree)?;
        let blocked = !blockers.is_empty();
        // Per-tree blockers only keep that tree; only missing global data
        // can prevent the sweep from removing the other finished trees.
        plan.add_tree(table, t, blocked, false);
    }
    plan.steps = plan
        .remove
        .iter()
        .map(|r| format!("remove worktree {}", r.tree))
        .collect();
    Ok(plan)
}

/// What `berbr done <task>` would do, checked against fresh Jira and GitLab: the
/// task's worktrees go (each must be finished: MR merged or closed, nothing
/// unpushed; uncommitted changes are lost once confirmed), my open tickets of
/// it are closed in Jira, the note gets `done: true` with the final MRs and
/// branches — after that berbr leaves it alone — and its herdr workspace
/// closes with every tab, agent and shell in it.
pub(crate) struct TaskPlan {
    table: Table,
    row: usize,
    /// What still holds the task open; none with force.
    pub(crate) blockers: Vec<String>,
    /// Worktrees with uncommitted changes, and how many: they go with them.
    dirty: Vec<(usize, usize)>,
    to_close: Vec<String>,
    left_open: Vec<String>,
}

impl TaskPlan {
    pub(crate) fn name(&self) -> &str {
        &self.table.tasks[self.row].task.name
    }

    /// One line per thing that will happen.
    pub(crate) fn steps(&self) -> Vec<String> {
        let row = &self.table.tasks[self.row];
        let mut steps: Vec<String> = row
            .trees
            .iter()
            .map(|&t| {
                let tree = self.table.tree(t);
                let name = tree.wt.name();
                let name = if row
                    .trees
                    .iter()
                    .filter(|&&i| self.table.tree(i).wt.name() == name)
                    .count()
                    > 1
                {
                    format!("{name} ({})", tree.wt.path.display())
                } else {
                    name
                };
                match self.changes(t) {
                    Some(n) => {
                        format!("! remove worktree {name}, losing {n} uncommitted change(s)")
                    }
                    None => format!("remove worktree {name}"),
                }
            })
            .collect();
        steps.extend(self.to_close.iter().map(|k| format!("close {k} in Jira")));
        steps.extend(
            self.left_open
                .iter()
                .map(|k| format!("{k} is not assigned to me — left open")),
        );
        steps.push("mark the note done".to_string());
        if row.space.is_some() {
            steps.push(format!(
                "close herdr workspace \"{}\"",
                row.task.space_name()
            ));
        }
        steps
    }

    fn report(&self, _force: bool) -> DonePlan {
        let row = &self.table.tasks[self.row];
        let mut plan = DonePlan {
            kind: "task",
            name: Some(self.name().to_string()),
            blockers: self.blockers.clone(),
            close_tickets: self.to_close.clone(),
            leave_tickets: self.left_open.clone(),
            note_done: true,
            close_space: row.space.is_some(),
            steps: self.steps(),
            dirty: self
                .dirty
                .iter()
                .map(|&(t, n)| DirtyTree {
                    tree: self.table.tree(t).wt.name(),
                    path: self.table.tree(t).wt.path.clone(),
                    uncommitted: n,
                })
                .collect(),
            ..Default::default()
        };
        // Task closing is atomic at the plan boundary: every checkout is a
        // conditional removal, including dirty trees held by another blocker.
        // The mod must show their loss warning before the blockers are cleared.
        for &t in &row.trees {
            plan.add_tree(&self.table, t, false, true);
        }
        plan
    }

    /// Whether running it loses uncommitted changes.
    pub(crate) fn loses_changes(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// Asked before the plan runs.
    pub(crate) fn question(&self) -> &'static str {
        if !self.loses_changes() {
            "Close it?"
        } else {
            "Close it, losing the uncommitted changes?"
        }
    }

    /// Uncommitted changes of worktree `tree`, if it has any.
    fn changes(&self, tree: usize) -> Option<usize> {
        self.dirty.iter().find(|(t, _)| *t == tree).map(|&(_, n)| n)
    }
}

/// `tasks` from the vault, `name` an open one of them by name or ticket.
pub(crate) fn plan_task(
    config: &Config,
    tasks: Vec<Task>,
    name: &str,
    force: bool,
) -> Result<TaskPlan, String> {
    plan_task_with(config, tasks, name, force, false)
}

fn plan_task_with(
    config: &Config,
    tasks: Vec<Task>,
    name: &str,
    force: bool,
    _read_only: bool,
) -> Result<TaskPlan, String> {
    let task = find_task(&tasks, name, false)?
        .filter(|&i| !tasks[i].done)
        .ok_or_else(|| format!("no open task {name}"))?;
    let id = tasks[task].name.clone();
    let table = table::build_closing(config, tasks, &id);
    let row = find_task(table.tasks.iter().map(|r| &r.task), name, false)?
        .filter(|&i| !table.tasks[i].task.done)
        .ok_or_else(|| format!("no open task \"{name}\""))?;
    plan_task_table(table, row, force)
}

fn plan_task_table(mut table: Table, row: usize, force: bool) -> Result<TaskPlan, String> {
    let task = &table.tasks[row];
    let mut blockers = Vec::new();
    for warning in table
        .warnings
        .iter()
        .chain(table.jira.warning.iter())
        .chain(table.gitlab.warning.iter())
    {
        if warning.contains(": ambiguous") && !warning.starts_with(&format!("{}:", task.task.name))
        {
            continue;
        }
        let blocker = format!("{warning} — resolve incomplete or ambiguous data before closing");
        if !blockers.contains(&blocker) {
            blockers.push(blocker);
        }
    }
    let mut dirty = Vec::new();
    let mut inspected = Vec::new();
    for &t in &task.trees {
        let tree = table.tree(t);
        if !force {
            blockers.extend(held(&table, tree));
        }
        let changes = uncommitted(tree)?;
        inspected.push((t, changes));
        if changes > 0 {
            dirty.push((t, changes));
        }
    }
    if !force {
        let held: Vec<usize> = task
            .trees
            .iter()
            .filter_map(|&t| table.tree(t).mr)
            .collect();
        for &m in task.mrs.iter().filter(|m| !held.contains(m)) {
            if table.mr(m).opened() {
                blockers.push(format!("MR !{} is open", table.mr(m).mr.iid()));
            }
        }
    }

    let open_tickets = |mine: bool| -> Vec<String> {
        task.task
            .tickets
            .iter()
            .chain(&task.found_tickets)
            .filter(|k| {
                table
                    .tickets
                    .get(*k)
                    .is_some_and(|t| t.mine == mine && !t.closed())
            })
            .cloned()
            .collect()
    };
    let (to_close, left_open) = (open_tickets(true), open_tickets(false));
    for (t, changes) in inspected {
        table.trees[t].changes = Some(changes);
    }
    Ok(TaskPlan {
        table,
        row,
        blockers,
        dirty,
        to_close,
        left_open,
    })
}

/// Does what the plan says, reporting each step to `say`.
pub(crate) fn finish_task(
    config: &Config,
    plan: TaskPlan,
    force: bool,
    say: &mut dyn FnMut(String),
) -> Result<(), String> {
    if !plan.blockers.is_empty() {
        return Err(not_finished(plan.name(), &plan.blockers));
    }
    let jira = if plan.to_close.is_empty() {
        None
    } else {
        Some(config.jira_client()?)
    };
    finish_task_with(plan, force, say, &mut |key| {
        close_in_jira(jira.as_ref().ok_or("Jira unavailable")?, key)
    })
}

fn finish_task_with(
    plan: TaskPlan,
    force: bool,
    say: &mut dyn FnMut(String),
    close_ticket: &mut dyn FnMut(&str) -> Result<String, String>,
) -> Result<(), String> {
    if !plan.blockers.is_empty() {
        return Err(not_finished(plan.name(), &plan.blockers));
    }
    let TaskPlan {
        table,
        row,
        dirty,
        to_close,
        ..
    } = plan;
    let row = &table.tasks[row];
    let mut problems = Vec::new();
    for &t in &row.trees {
        let tree = table.tree(t);
        let discard = dirty.iter().any(|&(d, _)| d == t);
        match remove_tree(tree, mr_of(&table, tree), force, discard, say) {
            Ok(kept) => problems.extend(kept),
            Err(e) => {
                let failure = format!("remove {} failed: {e}", tree.wt.name());
                say(failure.clone());
                problems.push(failure);
            }
        }
    }

    if !problems.is_empty() {
        return Err(format!(
            "task remains open; Jira, note and space unchanged:\n  {}",
            problems.join("\n  ")
        ));
    }

    for key in &to_close {
        match close_ticket(key) {
            Ok(status) => say(format!("{key} → {status}")),
            Err(e) => {
                let failure = format!("close {key} in Jira failed: {e}");
                say(failure.clone());
                problems.push(failure);
            }
        }
    }

    if !problems.is_empty() {
        return Err(format!(
            "task remains open; note and space unchanged:\n  {}",
            problems.join("\n  ")
        ));
    }

    let mrs: Vec<Value> = row
        .mrs
        .iter()
        .map(|&m| Value::String(table.mr(m).mr.web_url().to_string()))
        .collect();
    let mut branches: Vec<String> = row
        .mrs
        .iter()
        .map(|&m| table.mr(m).mr.source_branch().to_string())
        .collect();
    for &t in &row.trees {
        if let Some(b) = &table.tree(t).wt.branch
            && !branches.contains(b)
        {
            branches.push(b.clone());
        }
    }
    {
        let _lock = crate::vault::lock().map_err(err)?;
        let mut note = Note::read(&row.task.note.path).map_err(err)?;
        note.set("done", Value::Bool(true));
        note.set(
            "closed",
            Value::String(chrono::Local::now().format("%Y-%m-%d").to_string()),
        );
        if !mrs.is_empty() {
            note.set("mrs", Value::Sequence(mrs));
        }
        if !branches.is_empty() {
            note.set("branches", list(&branches));
        }
        note.save().map_err(err)?;
    }
    say(format!("\"{}\" is done", row.task.name));

    // last: this may close the tab wt itself runs in, and a popup over it;
    // whatever is in the task's workspace goes with it in one call
    let space = row.space.as_deref().filter(|_| herdr::available());
    for &t in &row.trees {
        close_herdr_views_outside(&table.tree(t).wt.path, space, &mut |e| problems.push(e));
    }
    if let Some(space) = space
        && let Err(e) = herdr::close_workspace(space)
    {
        problems.push(format!("can't close herdr workspace {space}: {e}"));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!("done, but:\n  {}", problems.join("\n  ")))
    }
}

/// `berbr done --tree <worktree>` and `berbr done --merged`: worktrees go one by one, each
/// once it is finished.
fn done_trees(config: &Config, args: DoneArgs) -> Result<(), String> {
    let (table, warning) = trees_for_done_with(config, args.force, args.dry_run)?;
    if let Some(warning) = warning {
        eprintln!("{warning}");
    }

    if !args.merged {
        let mut table = table;
        if let Some(repo) = &args.repo {
            let main = worktree::main_checkout(repo).map_err(err)?;
            table.trees.retain(|t| t.repo == main);
        }
        let tree = resolve_tree(&table, args.tree, &mut Terminal)?;
        let plan = TreePlan::new(table, tree, args.force)?;
        if args.dry_run {
            return plan.report(args.force).print(args.json);
        }
        if !plan.blockers.is_empty() {
            return Err(not_finished(&plan.name(), &plan.blockers));
        }
        if !args.yes && !confirm(&format!("{}?", plan.step()))? {
            return Ok(());
        }
        return finish_tree(plan, args.force, &mut print);
    }

    let plan = plan_sweep(&table, args.force)?;
    if args.dry_run {
        return plan.print(args.json);
    }
    if !plan.blockers.is_empty() {
        return Err(not_finished("worktrees", &plan.blockers));
    }
    if plan.trees.is_empty() {
        println!("No finished worktrees");
        return Ok(());
    }
    println!("Finished worktrees:");
    for step in &plan.steps {
        println!("  {step}");
    }
    if !args.yes && !confirm("Remove all of them?")? {
        return Ok(());
    }
    let mut removed = Vec::new();
    for t in plan.trees {
        let tree = table.tree(t);
        match remove_tree(tree, mr_of(&table, tree), false, false, &mut print) {
            Ok(kept) => {
                kept.into_iter().for_each(|k| eprintln!("{k}"));
                removed.push(tree);
            }
            Err(e) => eprintln!("{}: {e}", tree.wt.name()),
        }
    }
    // last: this may close the pane wt itself runs in
    for tree in removed {
        close_herdr_views(&tree.wt.path, &mut |e| eprintln!("{e}"));
    }
    Ok(())
}

/// Worktrees with their MRs, and what to warn about. GitLab did not answer: the
/// last known MR states will do, but with none at all an open MR looks like no
/// MR, so that takes force.
pub(crate) fn trees_for_done(
    config: &Config,
    force: bool,
) -> Result<(Table, Option<String>), String> {
    trees_for_done_with(config, force, false)
}

fn trees_for_done_with(
    config: &Config,
    force: bool,
    read_only: bool,
) -> Result<(Table, Option<String>), String> {
    if config.repos.is_empty() {
        return Err("no [[repo]] in ~/.config/berbr/config.toml".into());
    }
    let table = if read_only {
        table::build_read_only(config, Vec::new())
    } else {
        table::build(config, Vec::new(), true)
    };
    let warning = table.gitlab.warning.clone();
    if !read_only && let Some(blocker) = tree_data_blocker(&table, force) {
        return Err(blocker);
    }
    Ok((table, warning))
}

fn tree_data_blocker(table: &Table, force: bool) -> Option<String> {
    table
        .gitlab
        .warning
        .as_ref()
        .filter(|_| !force && table.gitlab.fresh == Fresh::Missing)
        .map(|warning| {
            format!("{warning} — not removing worktrees on partial data; restore GitLab access and retry")
        })
}

/// What `berbr done --tree <worktree>` would do: the worktree goes, with its branch and
/// herdr tabs, once nothing holds it.
pub(crate) struct TreePlan {
    table: Table,
    tree: usize,
    /// What still holds the worktree; none with force.
    pub(crate) blockers: Vec<String>,
}

impl TreePlan {
    /// `tree` indexes `table.trees`.
    pub(crate) fn new(table: Table, tree: usize, force: bool) -> Result<TreePlan, String> {
        let mut blockers = if force {
            Vec::new()
        } else {
            tree_blockers(&table, table.tree(tree))?
        };
        blockers.extend(tree_data_blocker(&table, force));
        Ok(TreePlan {
            table,
            tree,
            blockers,
        })
    }

    fn report(&self, force: bool) -> DonePlan {
        let mut plan = DonePlan {
            kind: "trees",
            blockers: self.blockers.clone(),
            steps: vec![self.step()],
            ..Default::default()
        };
        plan.add_tree(&self.table, self.tree, !self.blockers.is_empty(), force);
        plan
    }

    pub(crate) fn name(&self) -> String {
        self.table.tree(self.tree).wt.name()
    }

    pub(crate) fn step(&self) -> String {
        let wt = &self.table.tree(self.tree).wt;
        let branch = wt.branch.as_deref().unwrap_or("(detached)");
        format!("Remove {} (branch {branch}) and its herdr tabs", wt.name())
    }
}

pub(crate) fn finish_tree(
    plan: TreePlan,
    force: bool,
    say: &mut dyn FnMut(String),
) -> Result<(), String> {
    let tree = plan.table.tree(plan.tree);
    if let Some(kept) = remove_tree(tree, mr_of(&plan.table, tree), force, false, say)? {
        say(kept);
    }
    // last: this may close the pane wt itself runs in
    close_herdr_views(&tree.wt.path, say);
    Ok(())
}

/// What keeps a worktree from being removed: uncommitted changes, and what
/// `held` names.
fn tree_blockers(table: &Table, tree: &Tree) -> Result<Vec<String>, String> {
    let mut blockers = Vec::new();
    match uncommitted(tree)? {
        0 => {}
        n => blockers.push(format!("{}: {n} uncommitted change(s)", tree.wt.name())),
    }
    blockers.extend(held(table, tree));
    Ok(blockers)
}

fn uncommitted(tree: &Tree) -> Result<usize, String> {
    match tree.changes {
        Some(changes) => Ok(changes),
        None => worktree::changed_files(&tree.wt.path).map_err(err),
    }
}

/// What holds a worktree beyond its changes: an open MR, commits that are
/// nowhere but here or may be.
fn held(table: &Table, tree: &Tree) -> Vec<String> {
    let wt = tree.wt.name();
    let mut blockers = Vec::new();
    match (mr_of(table, tree), tree.unpushed) {
        (Some(mr), _) if mr.state() == MrState::Opened => {
            blockers.push(format!("{wt}: MR !{} is open", mr.iid()))
        }
        // git could not tell: they may be the only copy, MR or not
        (_, None) => blockers.push(format!("{wt}: unpushed commits unknown")),
        (Some(mr), Some(n)) if !branch_finished(mr, n, tree.head.as_deref()) => {
            blockers.push(format!("{wt}: commits after MR !{}", mr.iid()))
        }
        (None, Some(n)) if n > 0 => blockers.push(format!("{wt}: unpushed commits and no MR")),
        _ => {}
    }
    blockers
}

fn not_finished(name: &str, blockers: &[String]) -> String {
    format!(
        "\"{name}\" is not finished:\n  {}\nresolve these blockers before removing work",
        blockers.join("\n  ")
    )
}

fn mr_of<'a>(table: &'a Table, tree: &Tree) -> Option<&'a MergeRequest> {
    tree.mr.map(|i| &table.mr(i).mr)
}

/// Removes the worktree and deletes its branch; returns why the branch was kept,
/// when it was. `discard`: its uncommitted changes go with it, the branch
/// still only when nothing is lost.
fn remove_tree(
    tree: &Tree,
    mr: Option<&MergeRequest>,
    force: bool,
    discard: bool,
    say: &mut dyn FnMut(String),
) -> Result<Option<String>, String> {
    let name = tree.wt.name();
    let main = worktree::main_checkout(&tree.repo).map_err(err)?;
    worktree::remove_worktree(&main, &tree.wt.path, force || discard).map_err(err)?;
    say(format!("removed {name}"));
    let Some(branch) = &tree.wt.branch else {
        return Ok(None);
    };
    Ok(
        worktree::delete_branch(&main, branch, force || nothing_to_lose(tree, mr))
            .err()
            .map(|e| format!("{name}: branch {branch} kept: {e}")),
    )
}

/// Whether the branch may go though `git branch -d` refuses it: -d compares with
/// the local master, which is usually stale, and refuses squash-merged branches.
/// Nothing is lost when the MR is merged or every commit is on origin — not
/// when git could not count them.
fn nothing_to_lose(tree: &Tree, mr: Option<&MergeRequest>) -> bool {
    mr.is_some_and(|mr| mr.state() == MrState::Merged) || tree.unpushed == Some(0)
}

/// Moves the ticket to a "closed"-like status of its workflow.
fn close_in_jira(jira: &JiraClient, key: &str) -> Result<String, String> {
    let transitions = jira.transitions(key).map_err(err)?;
    let chosen =
        closing_transition(&transitions).ok_or("no closing transition from its current status")?;
    jira.transition(key, &chosen.id).map_err(err)?;
    Ok(chosen.to.name.clone())
}

/// A done-category transition, preferring "closed"/"done"/"resolved", never one
/// that cancels or declines.
fn closing_transition(transitions: &[Transition]) -> Option<&Transition> {
    let refused = ["cancel", "declin", "reject", "won't", "отмен", "отклон"];
    let preferred = ["close", "done", "resolve", "закры", "готов", "решен"];
    let name = |t: &Transition| format!("{} {}", t.name, t.to.name).to_lowercase();
    let usable: Vec<_> = transitions
        .iter()
        .filter(|t| t.to.status_category.key == table::Category::Done)
        .filter(|t| !refused.iter().any(|r| name(t).contains(r)))
        .collect();
    usable
        .iter()
        .find(|t| preferred.iter().any(|p| name(t).contains(p)))
        .or(usable.first())
        .copied()
}

pub(in crate::cli) fn cli_attach(
    config: &Config,
    key: &str,
    task: Option<String>,
) -> Result<(), String> {
    attach(config, key, task, &mut Terminal)
}

/// `berbr attach <KEY> [<task>]`: the ticket joins a task — just its `tickets:`,
/// no worktree or agent. Without a task, a picker over the open ones.
pub(crate) fn attach(
    config: &Config,
    key: &str,
    task: Option<String>,
    ui: &mut dyn Ui,
) -> Result<(), String> {
    if !is_ticket_key(key) {
        return Err(format!("\"{key}\" is not a ticket key"));
    }
    let tasks = load(config)?;
    if let Some(i) = find_task(&tasks, key, false)? {
        return Err(format!("{key} is already in \"{}\"", tasks[i].name));
    }
    let mut task = match task {
        Some(name) => resolve_task(config, tasks, Some(&name), ui)?,
        None => {
            let mut table = table::build(config, tasks, false);
            let open: Vec<usize> = (0..table.tasks.len())
                .filter(|&i| !table.tasks[i].task.done)
                .collect();
            let items: Vec<String> = open
                .iter()
                .map(|&i| table.tasks[i].task.name.clone())
                .collect();
            let chosen = open[ui.pick(&format!("Attach {key} to"), &items)?];
            table.tasks.swap_remove(chosen).task
        }
    };
    let _lock = crate::vault::lock().map_err(err)?;
    task.note.reload().map_err(err)?;
    let mut entries = task.note.strings("tickets");
    entries.push(key.to_string());
    task.note.set("tickets", list(&entries));
    task.note.save().map_err(err)?;
    ui.say(format!("{key} attached to \"{}\"", task.name));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        TaskPlan, TreePlan, closing_transition, nothing_to_lose, plan_sweep, plan_task_table,
        tree_blockers,
    };
    use crate::client::jira::Transition;
    use crate::table::fixtures::{mr, table, task, tree};

    #[test]
    fn closing_a_task_closes_its_workspace_last() {
        let mut table = table(vec![task("video", &[])], vec![], vec![], vec![]);
        table.tasks[0].space = Some("w1".into());
        let plan = TaskPlan {
            table,
            row: 0,
            blockers: vec![],
            dirty: vec![],
            to_close: vec![],
            left_open: vec![],
        };
        assert_eq!(
            plan.steps().last().unwrap(),
            "close herdr workspace \"video\""
        );
    }

    #[test]
    fn uncommitted_changes_are_lost_once_confirmed() {
        let mut busy = tree("busy", "demo/DEMO-1-a");
        busy.changes = Some(3);
        let mut table = table(vec![task("video", &[])], vec![], vec![], vec![busy]);
        table.tasks[0].trees = vec![0];
        let plan = TaskPlan {
            table,
            row: 0,
            blockers: vec![],
            dirty: vec![(0, 3)],
            to_close: vec![],
            left_open: vec![],
        };
        assert_eq!(
            plan.steps()[0],
            "! remove worktree busy, losing 3 uncommitted change(s)"
        );
        assert_eq!(plan.question(), "Close it, losing the uncommitted changes?");
    }

    #[test]
    fn unknown_unpushed_commits_hold_a_worktree() {
        let mut unknown = tree("unknown", "demo/DEMO-1-a");
        unknown.unpushed = None;
        let mut ahead = tree("ahead", "demo/DEMO-2-b");
        ahead.unpushed = Some(2);
        let table = table(
            vec![],
            vec![],
            vec![mr(1, "demo/DEMO-1-a", "x", "closed")],
            vec![unknown, ahead, tree("pushed", "demo/DEMO-3-c")],
        );
        let blockers =
            |name: &str| tree_blockers(&table, table.tree(table.tree_index(name))).unwrap();
        // with an MR or without one
        assert_eq!(
            blockers("unknown"),
            vec!["unknown: unpushed commits unknown"]
        );
        assert_eq!(blockers("ahead"), vec!["ahead: unpushed commits and no MR"]);
        assert!(blockers("pushed").is_empty());

        let unknown = table.tree(table.tree_index("unknown"));
        assert!(!nothing_to_lose(unknown, None));
        let closed = &table.mr(table.mr_index(1)).mr;
        assert!(!nothing_to_lose(unknown, Some(closed)));
        assert!(nothing_to_lose(
            table.tree(table.tree_index("pushed")),
            None
        ));
    }

    #[test]
    fn task_dry_plan_shares_blockers_tickets_and_steps() {
        use crate::table::{Category, fixtures::ticket};
        let mut dirty = tree("dirty", "DEMO-1-a");
        dirty.changes = Some(2);
        let mut open = tree("open", "DEMO-1-b");
        open.changes = Some(3);
        let table = table(
            vec![task("work", &["DEMO-1", "DEMO-2", "DEMO-3"])],
            vec![
                ticket("DEMO-1", Category::New, true),
                ticket("DEMO-2", Category::New, false),
                ticket("DEMO-3", Category::Done, true),
            ],
            vec![
                mr(1, "DEMO-1-a", "a", "merged"),
                mr(2, "DEMO-1-b", "b", "opened"),
            ],
            vec![dirty, open],
        );
        let plan = plan_task_table(table, 0, false).unwrap();
        let report = plan.report(false);
        assert_eq!(report.blockers, plan.blockers);
        assert_eq!(report.steps, plan.steps());
        assert_eq!(report.close_tickets, vec!["DEMO-1"]);
        assert_eq!(report.leave_tickets, vec!["DEMO-2"]);
        assert!(report.note_done);
        assert_eq!(report.remove[0].tree, "dirty");
        assert_eq!(report.remove[0].uncommitted, Some(2));
        assert!(report.keep.is_empty());
        assert_eq!(report.remove.len(), 2);
        assert_eq!(report.remove[1].uncommitted, Some(3));
        assert_eq!(report.dirty.len(), 2);
        assert!(report.blockers.iter().any(|b| b.contains("MR !2 is open")));
        let forced = plan_task_table(plan.table, 0, true).unwrap();
        assert!(forced.report(true).blockers.is_empty());
        assert_eq!(forced.report(true).remove.len(), 2);
    }

    #[test]
    fn a_blocked_finished_tree_does_not_block_the_sweep() {
        let mut table = table(
            vec![],
            vec![],
            vec![mr(1, "DEMO-1-a", "a", "merged")],
            vec![tree("dirty", "DEMO-1-a"), tree("clean", "DEMO-1-a")],
        );
        assert!(table.trees.iter().all(|t| t.finished));
        // Keep the finished classification, but introduce a per-tree blocker.
        let dirty = table.tree_index("dirty");
        let clean = table.tree_index("clean");
        table.trees[dirty].changes = Some(1);

        let plan = plan_sweep(&table, false).unwrap();
        let json = serde_json::to_value(&plan).unwrap();
        assert!(json["blockers"].as_array().unwrap().is_empty());
        assert_eq!(json["remove"].as_array().unwrap().len(), 1);
        assert_eq!(json["remove"][0]["tree"], "clean");
        assert_eq!(json["keep"].as_array().unwrap().len(), 1);
        assert_eq!(json["keep"][0]["tree"], "dirty");
        assert_eq!(json["keep"][0]["reason"], "uncommitted changes");
        // The real sweep executes exactly these indices.
        assert_eq!(plan.trees, vec![clean]);
        assert_eq!(plan.steps, vec!["remove worktree clean"]);
    }

    #[test]
    fn tree_and_sweep_plans_keep_the_same_unsafe_trees() {
        let mut dirty = tree("dirty", "DEMO-1-a");
        dirty.changes = Some(1);
        let mut ahead = tree("ahead", "DEMO-1-a");
        ahead.unpushed = Some(1);
        let mut unknown = tree("unknown", "DEMO-3-a");
        unknown.changes = None;
        unknown.unpushed = None;
        let table = table(
            vec![],
            vec![],
            vec![
                mr(1, "DEMO-1-a", "a", "merged"),
                mr(2, "DEMO-2-a", "b", "opened"),
            ],
            vec![
                dirty,
                ahead,
                tree("open", "DEMO-2-a"),
                tree("merged", "DEMO-1-a"),
                tree("no-mr", "misc"),
                unknown,
            ],
        );
        let plan = plan_sweep(&table, false).unwrap();
        assert_eq!(plan.remove.len(), 1);
        assert_eq!(plan.remove[0].tree, "merged");
        assert!(plan.blockers.is_empty());
        assert_eq!(
            plan.keep.iter().map(|k| k.reason).collect::<Vec<_>>(),
            vec![
                "uncommitted changes",
                "unpushed commits",
                "open MR",
                "not merged",
                "uncommitted changes"
            ]
        );
        let plan = TreePlan::new(table, 0, false).unwrap();
        assert_eq!(plan.report(false).keep[0].reason, "uncommitted changes");
        assert_eq!(plan.report(false).blockers, plan.blockers);
        let forced = TreePlan::new(plan.table, 0, true).unwrap();
        assert!(forced.report(true).keep.is_empty());
        assert_eq!(forced.report(true).remove.len(), 1);
        let empty = crate::table::fixtures::table(vec![], vec![], vec![], vec![]);
        assert!(plan_sweep(&empty, false).unwrap().remove.is_empty());
    }

    #[test]
    fn closes_rather_than_cancels() {
        let transitions: Vec<Transition> = serde_json::from_str(
            r#"[
              {"id":"601","name":"Cancelled","to":{"name":"Cancelled","statusCategory":{"key":"done"}}},
              {"id":"521","name":"In Progress","to":{"name":"В работе","statusCategory":{"key":"indeterminate"}}},
              {"id":"381","name":"Closed","to":{"name":"Закрыт","statusCategory":{"key":"done"}}}
            ]"#,
        )
        .unwrap();
        assert_eq!(closing_transition(&transitions).unwrap().id, "381");
        assert!(closing_transition(&transitions[..2]).is_none());
    }
}

#[cfg(test)]
mod regressions {
    use super::*;
    use crate::table::fixtures::{table, task_note, temp_dir, tree};
    fn config() -> Config {
        Config {
            name: "demo".into(),
            jira: None,
            gitlab: None,
            workflow: Default::default(),
            vault: crate::config::VaultConfig {
                path: temp_dir(),
                task_dirs: vec![],
                new_task_dir: "tasks".into(),
            },
            repos: vec![],
        }
    }
    #[test]
    fn task_close_warns_about_each_dirty_worktree() {
        let mut wt = tree("work", "work");
        wt.changes = Some(1);
        let table = table(
            vec![task_note("demo-0", "worktrees: [work]", "2026-11-01")],
            vec![],
            vec![],
            vec![wt],
        );
        let plan = plan_task_table(table, 0, false).unwrap();
        assert!(plan.blockers.is_empty());
        assert!(plan.steps()[0].contains("work, losing 1 uncommitted"));
        let report = plan.report(false);
        assert_eq!(report.dirty[0].tree, "work");
        assert_eq!(report.dirty[0].uncommitted, 1);
        assert_eq!(report.remove[0].uncommitted, Some(1));
    }
    #[test]
    fn failed_removal_must_leave_task_open() {
        let mut wt = tree("work", "work");
        wt.repo = temp_dir().join("absent-repo");
        let table = table(
            vec![task_note(
                "demo-0",
                "worktrees: [work]\ndone: false",
                "2026-11-01",
            )],
            vec![],
            vec![],
            vec![wt],
        );
        let path = table.tasks[0].task.note.path.clone();
        let plan = plan_task_table(table, 0, false).unwrap();
        assert!(plan.blockers.is_empty());
        let result = finish_task(&config(), plan, false, &mut |_| {});
        assert!(result.is_err());
        assert!(
            !Note::read(&path).unwrap().flag("done"),
            "failed worktree removal still writes done:true"
        );
    }
}

#[cfg(test)]
mod failure_tests {
    use super::*;
    use crate::table::fixtures::{table, task, ticket};

    #[test]
    fn failed_jira_step_leaves_note_and_space_open_and_reports_each_ticket() {
        let mut table = table(
            vec![task("demo-0", &["DEMO-1", "DEMO-2"])],
            vec![
                ticket("DEMO-1", table::Category::New, true),
                ticket("DEMO-2", table::Category::New, true),
            ],
            vec![],
            vec![],
        );
        table.tasks[0].space = Some("space-demo".into());
        let path = table.tasks[0].task.note.path.clone();
        let before = std::fs::read(&path).unwrap();
        let plan = plan_task_table(table, 0, false).unwrap();
        let mut calls = Vec::new();
        let mut report = Vec::new();
        let result = finish_task_with(plan, false, &mut |line| report.push(line), &mut |key| {
            calls.push(key.to_string());
            if key == "DEMO-1" {
                Ok("Done".into())
            } else {
                Err("transition refused".into())
            }
        });
        assert!(result.unwrap_err().contains("task remains open"));
        assert_eq!(calls, ["DEMO-1", "DEMO-2"]);
        assert!(report.iter().any(|l| l.contains("DEMO-1 → Done")));
        assert!(report.iter().any(|l| l.contains("DEMO-2 in Jira failed")));
        assert_eq!(std::fs::read(path).unwrap(), before);
    }
}

#[cfg(test)]
mod data_guard_tests {
    use super::*;
    use crate::table::fixtures::{table, task};

    #[test]
    fn incomplete_jira_data_blocks_finalization_even_with_bypass_flag() {
        let mut table = table(vec![task("demo-0", &["DEMO-1"])], vec![], vec![], vec![]);
        table.jira = table::Source {
            fresh: Fresh::Missing,
            warning: Some("Jira: incomplete data for DEMO-1".into()),
        };
        let plan = plan_task_table(table, 0, true).unwrap();
        assert!(
            plan.blockers
                .iter()
                .any(|b| b.contains("incomplete data for DEMO-1"))
        );
        let path = plan.table.tasks[0].task.note.path.clone();
        assert!(
            finish_task_with(
                plan,
                true,
                &mut |_| panic!("no steps allowed"),
                &mut |_| panic!("no Jira mutation allowed")
            )
            .is_err()
        );
        assert!(!Note::read(&path).unwrap().flag("done"));
    }

    #[test]
    fn refusal_messages_name_the_blockers_without_suggesting_a_bypass() {
        let error = not_finished("demo", &["demo: 2 uncommitted files".into()]);
        assert!(error.contains("2 uncommitted files"));
        assert!(!error.contains("--force"));
    }
}
