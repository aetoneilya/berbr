//! Opening worktree tabs and agents in herdr, closing them; the hidden
//! `berbr prepare`.

use super::brief::{agent_prompt, write_brief};
use super::common::{Ui, err};
use crate::cli::progress;
use crate::client::agent::AgentKind;
use crate::client::herdr::shell_quote;
use crate::client::{herdr, worktree};
use crate::vault::task::{Task, same_name, task_space_matches};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
pub(super) enum Space<'a> {
    Task(&'a Task),
    Label(&'a str),
}

impl fmt::Display for Space<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Task(task) => f.write_str(&task.space_name()),
            Self::Label(label) => f.write_str(label),
        }
    }
}

pub(super) struct Context<'a> {
    pub note: Option<&'a Path>,
    pub brief: &'a Path,
    pub description: Option<&'a str>,
    pub display_name: Option<&'a str>,
}

/// A fresh agent in the task's workspace: reads its context, then works on
/// a supplied description, or waits for a command.
pub(super) fn launch(
    kind: AgentKind,
    space: Space<'_>,
    cwd: &Path,
    label: &str,
    context: Context<'_>,
    focus: bool,
    ui: &mut dyn Ui,
) -> Result<(), String> {
    let prompt = agent_prompt(context.note, context.brief, context.description);
    let mut args: Vec<&str> = prompt.as_deref().into_iter().collect();
    if kind == AgentKind::Claude
        && let Some(name) = context.display_name
    {
        args.splice(0..0, ["--name", name]);
    }
    open_agent(kind, space, cwd, label, &args, focus, ui)
}

/// How `open_work` opens the worktree: with an agent or a shell only, and
/// whether its tab is focused.
pub(super) struct Open {
    pub(super) agent: bool,
    pub(super) kind: AgentKind,
    pub(super) focus: bool,
    pub(super) description: Option<String>,
}

/// What `berbr prepare` does in the new tab, handed over in a file.
#[derive(Serialize, Deserialize)]
struct PrepareSpec {
    main: PathBuf,
    name: String,
    branch: String,
    brief: String,
    note: Option<PathBuf>,
    pane: String,
    /// The agent to start once the worktree is ready; none, a shell only.
    agent: Option<String>,
    #[serde(default)]
    kind: AgentKind,
    #[serde(default)]
    description: Option<String>,
}

/// A tab for the worktree `name` in the task's herdr workspace. Fetch and
/// checkout of a big repo take half a minute, so they run in the tab itself:
/// `berbr prepare` shows the progress there and writes the brief, then the shell
/// cds into the worktree and starts the agent,
/// and `berbr start` returns at once. Without herdr it all happens here.
pub(super) fn open_work(
    task: &Task,
    main: &Path,
    name: &str,
    branch: &str,
    brief: String,
    open: Open,
    ui: &mut dyn Ui,
) -> Result<(), String> {
    let Open {
        agent,
        focus,
        kind,
        description,
    } = open;
    if task.tickets.is_empty() {
        let _lock = crate::vault::lock().map_err(err)?;
        let mut note = crate::vault::note::Note::read(&task.note.path).map_err(err)?;
        let mut names = note.strings("worktrees");
        if !names.iter().any(|existing| existing == name) {
            names.push(name.to_string());
            note.set("worktrees", super::common::list(&names));
            note.save().map_err(err)?;
        }
    }
    let (space, note) = (Space::Task(task), Some(task.note.path.as_path()));
    let dir = worktree::worktree_dir(main, name);
    if !herdr::available() {
        let dir = worktree::create_worktree(main, name, branch).map_err(err)?;
        ui.say(format!("worktree: {} ({branch})", dir.display()));
        let brief = write_brief(&dir, &brief)?;
        if agent {
            let prompt = agent_prompt(note, &brief, description.as_deref()).unwrap_or_default();
            ui.say(format!(
                "not in herdr — run it yourself: cd {} && {}",
                shell_quote(&dir.to_string_lossy()),
                kind.named_shell_command(name, &[&prompt])
            ));
        }
        return Ok(());
    }

    let cwd = if dir.exists() { &dir } else { main };
    let (tab, pane) = open_tab(space, cwd, name)?;
    let agent = agent.then(|| free_agent_name(name));
    let spec = PrepareSpec {
        main: main.to_path_buf(),
        name: name.to_string(),
        branch: branch.to_string(),
        brief,
        note: note.map(Path::to_path_buf),
        pane: pane.clone(),
        agent: agent.clone(),
        kind,
        description,
    };
    let spec_path =
        crate::config::cache_dir().join(format!("prepare-{pane}.json").replace(':', "-"));
    if let Some(parent) = spec_path.parent() {
        std::fs::create_dir_all(parent).map_err(err)?;
    }
    std::fs::write(&spec_path, serde_json::to_vec(&spec).map_err(err)?).map_err(err)?;

    let exe = std::env::current_exe().map_err(err)?;
    let prompt = prompt_path(&spec_path);
    let mut command = format!(
        "{} prepare {spec} && cd {dir}",
        shell_quote(&exe.to_string_lossy()),
        spec = shell_quote(&spec_path.to_string_lossy()),
        dir = shell_quote(&dir.to_string_lossy()),
    );
    if agent.is_some() {
        command.push_str(&format!(
            " && {agent_command} \"$(cat {prompt}; rm -f {prompt})\"",
            agent_command = kind.named_shell_prefix(name),
            prompt = shell_quote(&prompt.to_string_lossy()),
        ));
    }
    herdr::run_in_pane(&pane, &command).map_err(err)?;
    if focus {
        let _ = herdr::focus_tab(&tab);
    }
    ui.say(match &agent {
        Some(agent) => format!(
            "tab {name} → workspace \"{space}\": the worktree ({branch}) is being prepared there, then {kind} {agent} starts"
        ),
        None => format!(
            "tab {name} → workspace \"{space}\": the worktree ({branch}) is being prepared there"
        ),
    });
    Ok(())
}

/// Add waits for Git and the launch request, so a failed checkout is reported
/// to the calling agent rather than hidden in a new terminal.
pub(super) fn open_added_work(
    task: &Task,
    dir: &Path,
    name: &str,
    brief: String,
    open: Open,
    ui: &mut dyn Ui,
) -> Result<(), String> {
    let brief = write_brief(dir, &brief)?;
    let prompt = agent_prompt(Some(&task.note.path), &brief, open.description.as_deref());
    let (tab, pane) = open_tab(Space::Task(task), dir, name)?;
    let agent = free_agent_name(name);
    let args: Vec<&str> = prompt.as_deref().into_iter().collect();
    herdr::start_agent_with_label(open.kind, &agent, &pane, name, &args).map_err(err)?;
    if open.focus {
        herdr::focus_tab(&tab).map_err(err)?;
    }
    ui.say(format!(
        "worktree: {}; tab: {tab}; terminal: {pane}; agent: {agent}",
        dir.display()
    ));
    Ok(())
}

/// `berbr prepare <spec>`, run by `open_work` in the new tab: creates the worktree
/// with progress on screen, writes the brief and the first prompt; the tab's
/// shell then cds into the worktree and starts the agent.
pub(in crate::cli) fn cli_prepare(spec_path: &Path) -> Result<(), String> {
    let spec: PrepareSpec =
        serde_json::from_slice(&std::fs::read(spec_path).map_err(err)?).map_err(err)?;
    let dir = worktree::worktree_dir(&spec.main, &spec.name);
    let started = std::time::Instant::now();

    if !dir.exists() {
        println!("{} ({})", dir.display(), spec.branch);
        let plan = progress::step("worktree plan", || {
            worktree::plan_worktree(&spec.main, &spec.name, &spec.branch)
        })?;
        // a failed fetch leaves the worktree on the remote branch as last
        // fetched: better than no worktree
        if let Some(remote_branch) = &plan.fetch
            && let Err(e) = progress::step(&format!("git fetch origin {remote_branch}"), || {
                worktree::fetch(&spec.main, remote_branch)
            })
        {
            eprintln!("  ! {e}\n  going on from origin/{remote_branch} as last fetched");
        }
        progress::loud_step("git worktree add", || {
            worktree::add_worktree(&spec.main, &plan, true)
        })?;
        worktree::copy_worktree_includes(&spec.main, &dir);
    }
    let brief = write_brief(&dir, &spec.brief)?;
    let _ = std::fs::remove_file(spec_path);
    let Some(agent) = &spec.agent else {
        println!("ready in {:.0} s", started.elapsed().as_secs_f64());
        return Ok(());
    };
    println!(
        "ready in {:.0} s, starting {}",
        started.elapsed().as_secs_f64(),
        spec.kind
    );

    // the shell starts the agent from the worktree itself: herdr shows the branch
    // of the pane's shell, which would otherwise stay in the main checkout
    let prompt =
        agent_prompt(spec.note.as_deref(), &brief, spec.description.as_deref()).unwrap_or_default();
    std::fs::write(prompt_path(spec_path), prompt).map_err(err)?;
    let _ = herdr::name_agent_when_up(&spec.pane, agent);
    Ok(())
}

/// Where `berbr prepare` leaves the agent's first prompt for the shell to pass on.
fn prompt_path(spec_path: &Path) -> PathBuf {
    spec_path.with_extension("prompt")
}

/// Opens `cwd` as a tab in the herdr workspace `space` (created when missing),
/// starts a named agent there with `agent_args` and focuses it unless `focus`
/// is false.
pub(super) fn open_agent(
    kind: AgentKind,
    space: Space<'_>,
    cwd: &Path,
    label: &str,
    agent_args: &[&str],
    focus: bool,
    ui: &mut dyn Ui,
) -> Result<(), String> {
    if !herdr::available() {
        ui.say(format!(
            "not in herdr — run it yourself: cd {} && {}",
            shell_quote(&cwd.to_string_lossy()),
            kind.named_shell_command(label, agent_args)
        ));
        return Ok(());
    }
    let (tab, pane) = open_tab(space, cwd, label)?;
    let agent = free_agent_name(label);
    herdr::start_agent_with_label(kind, &agent, &pane, label, agent_args).map_err(err)?;
    if focus {
        let _ = herdr::focus_tab(&tab);
    }
    ui.say(format!(
        "agent {agent} → workspace \"{space}\", tab {label}"
    ));
    Ok(())
}

/// A new tab `label` in the herdr workspace `space`, created when missing;
/// returns the tab and its pane.
fn open_tab(space: Space<'_>, cwd: &Path, label: &str) -> Result<(String, String), String> {
    let existing = herdr::list_workspaces()
        .map_err(err)?
        .into_iter()
        .find(|w| match space {
            Space::Task(task) => task_space_matches(task, &w.label),
            Space::Label(label) => same_name(&w.label, label),
        });
    match existing {
        Some(ws) => herdr::create_tab(&ws.workspace_id, cwd, label).map_err(err),
        None => {
            let (tab, pane) = herdr::create_workspace(&space.to_string(), cwd).map_err(err)?;
            herdr::rename_tab(&tab, label).map_err(err)?;
            Ok((tab, pane))
        }
    }
}

fn free_agent_name(base: &str) -> String {
    let taken: Vec<String> = herdr::list_agents()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|a| a.name)
        .collect();
    herdr::free_agent_name(base, &taken)
}

/// herdr's agents; none outside herdr.
pub(super) fn live_agents() -> Vec<herdr::Agent> {
    if !herdr::available() {
        return Vec::new();
    }
    herdr::list_agents().unwrap_or_default()
}

/// Closes what herdr shows of a removed worktree: its own workspace, otherwise
/// the tabs whose agents work in it. Failures go to `say`.
pub(super) fn close_herdr_views(path: &Path, say: &mut dyn FnMut(String)) {
    close_herdr_views_outside(path, None, say);
}

/// `close_herdr_views`, leaving workspace `kept` alone: it is closed whole
/// later.
pub(super) fn close_herdr_views_outside(
    path: &Path,
    kept: Option<&str>,
    say: &mut dyn FnMut(String),
) {
    if !herdr::available() {
        return;
    }
    let path_str = path.to_string_lossy();
    let workspaces = herdr::list_workspaces().unwrap_or_default();
    let own: Vec<_> = workspaces
        .iter()
        .filter(|ws| Some(ws.workspace_id.as_str()) != kept)
        .filter(|ws| {
            ws.worktree
                .as_ref()
                .is_some_and(|wt| wt.checkout_path == path_str)
        })
        .collect();

    if !own.is_empty() {
        for ws in own {
            if let Err(err) = herdr::close_workspace(&ws.workspace_id) {
                say(format!("can't close workspace {}: {err}", ws.label));
            }
        }
        return;
    }

    // a tab in the task's workspace: close the tabs whose agents work in the worktree
    let mut tabs: Vec<String> = live_agents()
        .into_iter()
        .filter(|a| a.runs_in(path) && Some(a.workspace_id.as_str()) != kept)
        .map(|a| a.tab_id)
        .collect();
    tabs.dedup();
    for tab in tabs {
        if let Err(err) = herdr::close_tab(&tab) {
            say(format!("can't close tab {tab}: {err}"));
        }
    }
}
