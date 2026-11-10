//! Opening worktree tabs and agents in herdr, closing them; the hidden
//! `berbr prepare`.

use super::brief::{agent_prompt, write_brief};
use super::common::{Ui, err};
use crate::cli::progress;
use crate::client::agent::AgentKind;
use crate::client::herdr::shell_quote;
use crate::client::{herdr, worktree};
use crate::vault::task::{Task, same_name, task_space};
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

/// The task directory is needed for atomic note replacement; the cache holds
/// the shared note-write lock. Do not grant access to the entire vault/home.
fn task_agent_args(
    kind: AgentKind,
    note: Option<&Path>,
    args: &[&str],
) -> Result<Vec<String>, String> {
    let mut result = Vec::new();
    if kind == AgentKind::Codex
        && let Some(parent) = note.and_then(Path::parent)
    {
        let cache = crate::config::cache_dir();
        std::fs::create_dir_all(&cache).map_err(err)?;
        for dir in [parent, cache.as_path()] {
            let dir = dir.canonicalize().map_err(err)?;
            result.extend(["--add-dir".into(), dir.to_string_lossy().into_owned()]);
        }
    }
    result.extend(args.iter().map(|s| (*s).to_string()));
    Ok(result)
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
    pub(super) directory: PathBuf,
    pub(super) agent: bool,
    pub(super) kind: AgentKind,
    pub(super) focus: bool,
    pub(super) description: Option<String>,
}

/// What `berbr prepare` does in the new tab, handed over in a file.
#[derive(Serialize, Deserialize)]
struct PrepareSpec {
    #[serde(default = "legacy_directory")]
    directory: PathBuf,
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

fn legacy_directory() -> PathBuf {
    worktree::LEGACY_DIRECTORY.into()
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
    let directory = open.directory.clone();
    worktree::validate_name(name).map_err(err)?;
    worktree::validate_directory(&directory).map_err(err)?;
    if worktree::existing_worktree(main, &directory, name)
        .map_err(err)?
        .is_some()
    {
        return Err(format!(
            "worktree {name} already exists; select the owning task to resume it"
        ));
    }
    let Open {
        agent,
        focus,
        kind,
        description,
        ..
    } = open;
    {
        let _lock = crate::vault::lock().map_err(err)?;
        let mut note = crate::vault::note::Note::read(&task.note.path).map_err(err)?;
        let mut names = note.strings("worktrees");
        let reference = worktree::worktree_dir(main, &directory, name)
            .to_string_lossy()
            .into_owned();
        if !names.contains(&reference) {
            names.push(reference);
            note.set("worktrees", super::common::list(&names));
            note.save().map_err(err)?;
        }
    }
    let (space, note) = (Space::Task(task), Some(task.note.path.as_path()));
    let dir = worktree::worktree_dir(main, &directory, name);
    if !herdr::available() {
        let dir = worktree::create_new_worktree(main, &directory, name, branch).map_err(err)?;
        ui.say(format!("worktree: {} ({branch})", dir.display()));
        let brief = write_brief(&dir, &brief)?;
        if agent {
            let prompt = agent_prompt(note, &brief, description.as_deref()).unwrap_or_default();
            let args = task_agent_args(kind, note, &[&prompt])?;
            let args: Vec<_> = args.iter().map(String::as_str).collect();
            ui.say(format!(
                "not in herdr — run it yourself: cd {} && {}",
                shell_quote(&dir.to_string_lossy()),
                kind.named_shell_command(name, &args)
            ));
        }
        return Ok(());
    }

    let cwd = if dir.exists() { &dir } else { main };
    let (tab, pane) = open_tab(space, cwd, name, ui)?;
    let agent = agent.then(|| free_agent_name(name));
    let spec = PrepareSpec {
        directory,
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
        let args = task_agent_args(kind, note, &[])?;
        let args: Vec<_> = args.iter().map(String::as_str).collect();
        command.push_str(&format!(
            " && {agent_command} \"$(cat {prompt}; rm -f {prompt})\"",
            agent_command = kind.named_shell_command(name, &args),
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
    let (tab, pane) = open_tab(Space::Task(task), dir, name, ui)?;
    let agent = free_agent_name(name);
    let prompt_args: Vec<&str> = prompt.as_deref().into_iter().collect();
    let args = task_agent_args(open.kind, Some(&task.note.path), &prompt_args)?;
    let args: Vec<_> = args.iter().map(String::as_str).collect();
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
    worktree::validate_name(&spec.name).map_err(err)?;
    let dir = worktree::worktree_dir(&spec.main, &spec.directory, &spec.name);
    worktree::ensure_excluded(&spec.main, &spec.directory).map_err(err)?;
    let started = std::time::Instant::now();

    if dir.symlink_metadata().is_ok() {
        return Err(format!(
            "{} already exists; refusing to replace its brief",
            dir.display()
        ));
    }
    {
        println!("{} ({})", dir.display(), spec.branch);
        let plan = progress::step("worktree plan", || {
            worktree::plan_worktree(&spec.main, &spec.directory, &spec.name, &spec.branch)
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
    worktree::ensure_excluded(&spec.main, &spec.directory).map_err(err)?;
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
    open_agent_with_role(kind, space, cwd, label, agent_args, focus, None, ui)
}

pub(super) fn open_agent_with_role(
    kind: AgentKind,
    space: Space<'_>,
    cwd: &Path,
    label: &str,
    agent_args: &[&str],
    focus: bool,
    role: Option<&str>,
    ui: &mut dyn Ui,
) -> Result<(), String> {
    let note = match space {
        Space::Task(task) => Some(task.note.path.as_path()),
        Space::Label(_) => None,
    };
    let args = task_agent_args(kind, note, agent_args)?;
    let agent_args: Vec<_> = args.iter().map(String::as_str).collect();
    let agent_args = agent_args.as_slice();
    if !herdr::available() {
        ui.say(format!(
            "not in herdr — run it yourself: cd {} && {}",
            shell_quote(&cwd.to_string_lossy()),
            kind.named_shell_command(label, agent_args)
        ));
        return Ok(());
    }
    if let Space::Task(task) = space
        && open_in_project_space(kind, task, cwd, label, agent_args, focus, role, ui)?.is_some()
    {
        return Ok(());
    }
    let (tab, pane) = open_tab(space, cwd, label, ui)?;
    let agent = free_agent_name(label);
    if let Some(role) = role {
        herdr::label_role(&pane, role).map_err(err)?;
    }
    herdr::start_agent_with_label(kind, &agent, &pane, label, agent_args).map_err(err)?;
    if focus {
        let _ = herdr::focus_tab(&tab);
    }
    ui.say(format!(
        "agent {agent} → workspace \"{space}\", tab {label}"
    ));
    Ok(())
}

/// Herdr App lists a worktree's space under the space of its repo's main
/// checkout, so the project gets one before work opens in a space of its
/// own: a shell in the main checkout, named `project`. Never switches the
/// checkout's branch (it may hold someone's changes), only says when it is
/// not the default one.
/// Returns that space and its first terminal; none outside Herdr App.
pub(super) fn ensure_project_space(
    main: &Path,
    project: &str,
    ui: &mut dyn Ui,
) -> Result<Option<herdr::SpacePlace>, String> {
    if !herdr::nests_worktree_spaces() {
        return Ok(None);
    }
    let places = herdr::space_places().map_err(err)?;
    if let Some(found) = project_space(main, &places, |dir| worktree::worktree_root(dir).ok()) {
        return Ok(Some(found.clone()));
    }
    let (_, terminal) = herdr::create_workspace(project, main).map_err(err)?;
    ui.say(format!("space \"{project}\" → {}", main.display()));
    let default = worktree::Refs::read(main)
        .ok()
        .and_then(|refs| refs.default_branch(None))
        .map(|b| b.trim_start_matches("origin/").to_string());
    if let (Some(default), Ok(current)) = (default, worktree::current_branch(main))
        && default != current
    {
        ui.say(format!(
            "  ! the main checkout is on {current}, not {default}"
        ));
    }
    let space = herdr::space_places()
        .map_err(err)?
        .into_iter()
        .find(|p| p.terminal.as_deref() == Some(terminal.as_str()));
    Ok(space)
}

/// The first space (by where it is now) in the main checkout `main` itself,
/// not in one of its linked worktrees. `root_of` gives the git checkout a
/// folder is in.
fn project_space<'a>(
    main: &Path,
    places: &'a [herdr::SpacePlace],
    root_of: impl Fn(&Path) -> Option<PathBuf>,
) -> Option<&'a herdr::SpacePlace> {
    let main = canonical(main);
    places.iter().find(|p| {
        let place = canonical(&p.place);
        place.starts_with(&main) && root_of(&place).is_some_and(|root| canonical(&root) == main)
    })
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Herdr App: a task's agent in the main checkout (no worktree) gets no space
/// of its own: it is a pane beside the project space's terminal, labelled with
/// the task so the board still finds it. None when that does not apply.
fn open_in_project_space(
    kind: AgentKind,
    task: &Task,
    cwd: &Path,
    label: &str,
    agent_args: &[&str],
    focus: bool,
    role: Option<&str>,
    ui: &mut dyn Ui,
) -> Result<Option<()>, String> {
    if !herdr::nests_worktree_spaces() {
        return Ok(None);
    }
    let Ok(main) = worktree::main_checkout(cwd) else {
        return Ok(None);
    };
    if worktree::worktree_root(cwd).map(|r| canonical(&r)).ok() != Some(canonical(&main)) {
        return Ok(None);
    }
    let project = project_label(task, &main);
    let Some(space) = ensure_project_space(&main, &project, ui)? else {
        return Ok(None);
    };
    let Some(terminal) = &space.terminal else {
        return Ok(None);
    };
    let pane = herdr::split_pane(terminal, "right", cwd).map_err(err)?;
    herdr::label_task(&pane, &task.name).map_err(err)?;
    if let Some(role) = role {
        herdr::label_role(&pane, role).map_err(err)?;
    }
    let agent = free_agent_name(label);
    herdr::start_agent_with_label(kind, &agent, &pane, label, agent_args).map_err(err)?;
    if focus {
        let _ = herdr::focus_pane(&space.space, &pane);
    }
    ui.say(format!(
        "agent {agent} → a pane in space \"{project}\" ({})",
        cwd.display()
    ));
    Ok(Some(()))
}

/// The project a task's space is filed under: the note's `project`, else the
/// main checkout's folder name.
fn project_label(task: &Task, main: &Path) -> String {
    task.note
        .string("project")
        .filter(|p| !p.trim().is_empty())
        .or_else(|| main.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "project".into())
}

/// `berbr open <worktree> [--shell]`: work in a worktree that belongs to no
/// task (the mod's Cleanup rows). It gets a space named after its branch,
/// filed under the project's space; each call adds a tab with a new agent,
/// or a shell, there.
pub(in crate::cli) fn cli_open(
    config: &crate::config::Config,
    path: &Path,
    shell: bool,
    resume: Option<&str>,
    role: Option<&str>,
) -> Result<(), String> {
    let ui: &mut dyn Ui = &mut super::common::Terminal;
    let path = path
        .canonicalize()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let root = worktree::worktree_root(&path).map_err(err)?;
    if root.canonicalize().ok().as_deref() != Some(path.as_path()) {
        return Err(format!(
            "{} is not the top of a git worktree",
            path.display()
        ));
    }
    if let Some(session) = resume {
        return super::select::resume_in_tree(config, &path, session, role, ui);
    }
    let main = worktree::main_checkout(&path).map_err(err)?;
    let label = worktree::current_branch(&path).unwrap_or_else(|_| {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "worktree".into())
    });
    let kind = config
        .workflow
        .agent_override
        .unwrap_or(config.workflow.agent);
    if !herdr::available() {
        if !shell {
            ui.say(format!(
                "not in herdr — run it yourself: cd {} && {}",
                shell_quote(&path.to_string_lossy()),
                kind.named_shell_command(&label, &[])
            ));
        }
        return Ok(());
    }
    let _operation = herdr::operation();
    if shell {
        let table = crate::table::build(config, crate::cli::load(config)?, false);
        if let Some(task) = table
            .tasks
            .iter()
            .find(|row| row.trees.iter().any(|&i| table.tree(i).wt.path == path))
        {
            let (tab, _) = open_tab(Space::Task(&task.task), &path, "shell", ui)?;
            let _ = herdr::focus_tab(&tab);
            ui.say(format!(
                "shell → task \"{}\" ({})",
                task.task.space_name(),
                path.display()
            ));
            return Ok(());
        }
    }
    if let Err(e) = ensure_project_space(&main, &config.name, ui) {
        ui.say(format!("  ! no project space: {e}"));
    }
    let spaces = herdr::list_workspaces().map_err(err)?;
    let places = herdr::space_places().unwrap_or_default();
    let existing = spaces.iter().find(|s| {
        same_name(&s.label, &label)
            || places
                .iter()
                .any(|p| p.space == s.workspace_id && canonical(&p.place) == path)
    });
    let tab_label = if shell {
        "shell".to_string()
    } else {
        kind.to_string()
    };
    let (tab, pane) = match existing {
        Some(space) => herdr::create_tab(&space.workspace_id, &path, &tab_label).map_err(err)?,
        None => {
            let (tab, pane) = herdr::create_workspace(&label, &path).map_err(err)?;
            herdr::rename_tab(&tab, &tab_label).map_err(err)?;
            (tab, pane)
        }
    };
    if !shell {
        let agent = free_agent_name(&label);
        herdr::start_agent_with_label(kind, &agent, &pane, &label, &[]).map_err(err)?;
    }
    let _ = herdr::focus_tab(&tab);
    ui.say(format!(
        "{tab_label} → space \"{label}\" ({})",
        path.display()
    ));
    Ok(())
}

/// A new tab `label` in the herdr workspace `space`, created when missing;
/// returns the tab and its pane. A new task space first gets its project's
/// space (see `ensure_project_space`).
fn open_tab(
    space: Space<'_>,
    cwd: &Path,
    label: &str,
    ui: &mut dyn Ui,
) -> Result<(String, String), String> {
    let spaces = herdr::list_workspaces().map_err(err)?;
    let existing = match space {
        Space::Task(task) => task_space(task, &spaces)?,
        Space::Label(label) => {
            let matches: Vec<_> = spaces
                .iter()
                .filter(|w| same_name(&w.label, label))
                .collect();
            match matches.as_slice() {
                [] => None,
                [space] => Some(*space),
                _ => return Err(format!("ambiguous Herdr space {label}")),
            }
        }
    };
    match existing {
        Some(ws) => herdr::create_tab(&ws.workspace_id, cwd, label).map_err(err),
        None => {
            if let Space::Task(task) = space
                && let Ok(main) = worktree::main_checkout(cwd)
                && let Err(e) = ensure_project_space(&main, &project_label(task, &main), ui)
            {
                ui.say(format!("  ! no project space: {e}"));
            }
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
        .filter(|a| a.occupies_pane() && a.runs_in(path) && Some(a.workspace_id.as_str()) != kept)
        .map(|a| a.tab_id)
        .collect();
    tabs.dedup();
    for tab in tabs {
        if let Err(err) = herdr::close_tab(&tab) {
            say(format!("can't close tab {tab}: {err}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::project_space;
    use crate::client::herdr::SpacePlace;
    use std::path::{Path, PathBuf};

    #[test]
    fn project_space_is_a_space_in_the_main_checkout_not_in_its_worktrees() {
        let main = Path::new("/nonexistent/repo");
        let tree = main.join(".herdr/worktrees/demo-1");
        // A worktree lives inside the main checkout's folder: git, not the path, tells them apart.
        let root_of = |dir: &Path| {
            Some(if dir.starts_with(&tree) {
                tree.clone()
            } else {
                main.to_path_buf()
            })
        };
        let place = |space: &str, p: &Path| SpacePlace {
            space: space.into(),
            place: p.to_path_buf(),
            terminal: Some(format!("t-{space}")),
        };
        let found =
            |places: &[SpacePlace]| project_space(main, places, root_of).map(|p| p.space.clone());
        assert_eq!(found(&[]), None);
        assert_eq!(
            found(&[place("a", &tree), place("b", &tree.join("src"))]),
            None
        );
        assert_eq!(found(&[place("a", Path::new("/nonexistent/other"))]), None);
        assert_eq!(
            found(&[
                place("a", &tree),
                place("b", &main.join("src")),
                place("c", main)
            ]),
            Some("b".into())
        );
        let not_a_repo = |_: &Path| None::<PathBuf>;
        assert!(project_space(main, &[place("a", main)], not_a_repo).is_none());
    }
}
