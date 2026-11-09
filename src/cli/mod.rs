use crate::client::agent::AgentKind;
mod app;
mod migrate;
mod progress;
mod table;
pub(crate) mod work;

use crate::config::{Config, choose_project, load_projects, project_of};
use crate::vault::task::Task;
use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::{Shell, generate};
use std::io::{self, IsTerminal};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "berbr", author, version, long_about = None)]
/// Tasks, Jira tickets, merge requests and worktrees in one table
pub struct Cli {
    /// The project; default: the one the current directory is in, else the first
    #[arg(long, short = 'p', global = true)]
    project: Option<String>,
    /// CLI for new work; on select, focus/resume only this CLI's conversations
    #[arg(long, global = true, value_enum)]
    agent: Option<AgentKind>,
    /// Without a command: the TUI
    #[command(subcommand)]
    command: Option<CliCommands>,
}

#[derive(Subcommand, Debug)]
enum CliCommands {
    /// Resolve local project/task/worktree context without network or cache writes.
    /// Unresolved or ambiguous context is reported with candidates, never guessed.
    Context {
        #[arg(long)]
        json: bool,
        /// Explicit task ID or title within the selected project
        #[arg(long)]
        task: Option<String>,
    },
    /// Read-only JSON board for the native Herdr App mod.
    AppSnapshot {
        /// Read cached Jira/GitLab data without network calls.
        #[arg(long)]
        cached: bool,
    },
    /// Show universal settings as JSON, or save defaults for every project.
    Settings {
        #[arg(long, value_enum)]
        default_agent: Option<AgentKind>,
        #[arg(long = "task-naming-agent", alias = "generation-agent", value_enum)]
        generation_agent: Option<AgentKind>,
        /// Empty string selects the Task naming CLI's default model.
        #[arg(long = "task-naming-model", alias = "generation-model")]
        generation_model: Option<String>,
    },
    /// (internal) Open the Herdr plugin's board without loading project config.
    #[command(hide = true)]
    PluginOpen,
    /// (internal) Start the sidebar metadata worker for this Herdr session.
    #[command(hide = true)]
    PluginMetadataStart,
    /// (internal) Publish sidebar metadata without writing task notes.
    #[command(hide = true)]
    PluginMetadata {
        #[arg(long)]
        once: bool,
        #[arg(long)]
        cached: bool,
    },
    /// The TUI, as `berbr` without a command
    Tui {},

    /// Start new work. `berbr start "<task name>"`: a task note, its herdr workspace
    /// and a worktree tab with the selected agent briefed from the note and the first ticket.
    /// `berbr start DEMO-123`: the same, the task named after the ticket's summary
    Start(StartArgs),
    /// Open a worktree tab and a new agent in the task's Herdr space. Use --name
    /// for another branch of a ticket already in the task, or --in for a fresh
    /// agent in an existing worktree. KEY is optional with --name or --in. Without a TTY, never
    /// prompts or focuses the new tab; success prints its path, tab and terminal.
    Add(AddArgs),
    /// Put a ticket into a task without starting work on it (no worktree)
    Attach {
        key: String,
        /// The task; a picker when omitted
        task: Option<String>,
    },
    /// Go back to a task: its agent, or its CLI resuming the last session there
    Select(SelectArgs),
    /// (internal) Run in a new tab by start/add: create the worktree, then the agent
    #[command(hide = true)]
    Prepare { spec: PathBuf },
    /// Append a dated finding to the task note's "Agent notes". Resolves the
    /// current worktree/space or --task; never prompts. Prints the task ID.
    Note {
        /// Explicit task ID or title; defaults to the current worktree/workspace
        #[arg(long)]
        task: Option<String>,
        #[arg(trailing_var_arg = true, required = true)]
        text: Vec<String>,
    },
    /// Rename a task's title and existing Herdr space; never renames worktrees
    /// or branches. No prompts. Defaults to the current worktree/space's task.
    Rename {
        /// [<task ID or old title>] <new title>
        #[arg(num_args = 1..=2, required = true)]
        args: Vec<String>,
        /// Also rename the calling terminal's tab; skip if it cannot be identified
        #[arg(long)]
        tab: Option<String>,
    },
    /// Show or update task context as JSON (id, title, description, note, done).
    /// Defaults to the current worktree/space; pass an ID/title otherwise.
    /// Never prompts. Updates preserve other note fields and Agent notes.
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    /// Tasks with their tickets, merge requests, worktrees and agents; then work
    /// that belongs to no task. Updates `state` and `sessions` in task notes.
    Status(StatusArgs),
    /// Start a reviewer agent next to the task's agent
    Review { name: Option<String> },
    /// Open a worktree that belongs to no task: a new agent (or a shell) in a
    /// space named after its branch, filed under the project's space
    Open {
        /// The worktree's folder
        path: PathBuf,
        /// A shell, no agent
        #[arg(long)]
        shell: bool,
    },
    /// Close a task: `berbr done "<task>"` (or a ticket of it) removes its finished
    /// worktrees, closes my tickets in Jira and marks the note done. Use --tree
    /// to remove one checkout, or omit the target inside that checkout
    Done(DoneArgs),
    /// The tail of berbr's log and the Herdr.app mod's log. Changes are always
    /// logged; set `debug = true` in the config or BERBR_DEBUG=1 for everything
    Logs {
        /// Lines from each log
        #[arg(long, short = 'n', default_value_t = 40)]
        lines: usize,
        /// Only print where the logs are
        #[arg(long)]
        path: bool,
    },
    /// Copy task notes out of the Obsidian vault into ~/.berbr/projects/<project>/tasks
    /// (closed ones into done/) and write ~/.berbr/config.toml without [vault].
    /// The vault's notes stay until you delete them
    Migrate {
        /// Only say what would be copied
        #[arg(long)]
        dry_run: bool,
    },
    /// Shell completion script
    Completions {
        #[arg(value_enum)]
        shell: Shell,
    },
}

// The arguments of the bigger commands, handed to them whole. No doc comments
// on the structs: clap would take them for the commands' help.

#[derive(Args, Debug, Default)]
pub(crate) struct StartArgs {
    /// Task name or ticket key; a picker when omitted
    pub(crate) target: Option<String>,
    /// Create without a title; the agent names it after your first message
    #[arg(long, conflicts_with = "target")]
    pub(crate) unnamed: bool,
    /// Tickets of the new task, the first one gets the worktree
    #[arg(long = "ticket", short = 'k')]
    pub(crate) tickets: Vec<String>,
    #[arg(long = "tag", short = 't')]
    pub(crate) tags: Vec<String>,
    /// The goal: first line of the note and the agent's brief
    #[arg(long, short = 'm')]
    pub(crate) message: Option<String>,
    /// Task description: saved in the note and given to the agent to work on.
    /// With no task name, the agent generates a name from this text first
    #[arg(long)]
    pub(crate) description: Option<String>,
    /// Worktree, tab and agent name; checkout root: [workflow] worktree_dir
    /// (default .herdr/worktrees). Name defaults to ticket summary/title/task ID
    #[arg(long, short = 'n')]
    pub(crate) name: Option<String>,
    /// Default: <branch_prefix><KEY>-<name>
    #[arg(long, short = 'b')]
    pub(crate) branch: Option<String>,
    /// Don't switch to the new tab
    #[arg(long)]
    pub(crate) bg: bool,
    /// Create a Jira ticket named after the task, assigned to me
    /// (the project's `jira.project`, `jira.issue_type`)
    #[arg(long)]
    pub(crate) new_ticket: bool,
    /// No worktree: the agent works in the main checkout
    #[arg(long)]
    pub(crate) no_worktree: bool,
    /// No agent: the worktree tab gets a shell. With --no-worktree too, only
    /// the note
    #[arg(long)]
    pub(crate) no_agent: bool,
}

#[derive(Subcommand, Debug)]
pub(crate) enum TaskCommand {
    /// List local task IDs/titles without network, cache or task-note writes
    List {
        #[arg(long)]
        json: bool,
        /// Include closed tasks
        #[arg(long, short = 'a')]
        all: bool,
    },
    /// Show the task as JSON; accepts its stable ID or title
    Show { task: Option<String> },
    /// Update only the supplied fields, preserving the rest of the note
    Update {
        task: Option<String>,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, conflicts_with = "description_file")]
        description: Option<String>,
        /// Read multiline description from a UTF-8 file, or '-' for stdin
        #[arg(long)]
        description_file: Option<PathBuf>,
        /// Fill missing fields only; keep existing user text and title
        #[arg(long)]
        fill_empty: bool,
    },
}

#[derive(Args, Debug, Default)]
pub(crate) struct AddArgs {
    /// [<task>] [<KEY>]; task defaults to the current worktree/space; omit KEY with --name or --in
    #[arg(num_args = 0..=2)]
    pub(crate) args: Vec<String>,
    /// Run another agent on the same branch in this existing task worktree
    #[arg(long = "in", conflicts_with = "name")]
    pub(crate) in_tree: Option<String>,
    /// New worktree, branch suffix, tab and agent name; required for an existing key
    #[arg(long, short = 'n')]
    pub(crate) name: Option<String>,
    /// Context to include in the brief (without --description the agent waits)
    #[arg(long, short = 'm')]
    pub(crate) message: Option<String>,
    /// A description for the agent to work on immediately
    #[arg(long)]
    pub(crate) description: Option<String>,
    /// Keep the caller's focus; automatic when stdin is not a TTY
    #[arg(long)]
    pub(crate) bg: bool,
}

#[derive(Args, Debug, Default)]
pub(crate) struct SelectArgs {
    /// Task name or ticket key; a picker when omitted
    pub(crate) target: Option<String>,
    /// That worktree of the task; for a task without worktrees or sessions,
    /// the name of its first one
    #[arg(long)]
    pub(crate) tree: Option<String>,
    /// Pick which of the task's sessions to resume
    #[arg(long)]
    pub(crate) session: bool,
    /// Start a fresh conversation, even if an agent or saved session exists
    #[arg(long = "new", conflicts_with = "session")]
    pub(crate) new_session: bool,
    /// Don't switch to it
    #[arg(long)]
    pub(crate) bg: bool,
}

#[derive(Args, Debug)]
struct StatusArgs {
    /// Only tasks with this tag
    #[arg(long, short = 't')]
    tag: Option<String>,
    /// Also done tasks, tickets with their status, merged MRs, finished
    /// worktrees and tickets without activity
    #[arg(long, short = 'a')]
    all: bool,
    /// Don't update task notes
    #[arg(long)]
    no_write: bool,
}

#[derive(Args, Debug)]
struct DoneArgs {
    /// Show the plan without changing files, Jira or herdr
    #[arg(long)]
    dry_run: bool,
    /// Print the dry-run plan as one JSON object
    #[arg(long, requires = "dry_run")]
    json: bool,
    /// Task ID, title or ticket alias; checkout removal requires --tree
    #[arg(conflicts_with_all = ["tree", "merged"])]
    name: Option<String>,
    /// Remove only this worktree; never resolve a task or ticket alias
    #[arg(long, conflicts_with = "merged")]
    tree: Option<String>,
    /// Main repository path to disambiguate --tree
    #[arg(long, requires = "tree")]
    repo: Option<PathBuf>,
    /// Remove every clean worktree whose MR is merged or closed
    #[arg(long)]
    merged: bool,
    /// Skip the uncommitted / open MR / unpushed checks
    #[arg(long, short = 'f')]
    force: bool,
    #[arg(long, short = 'y')]
    yes: bool,
}

pub fn run() -> Result<(), String> {
    let cli = Cli::parse();
    if let Some(CliCommands::Logs { lines, path }) = cli.command {
        return logs(lines, path);
    }
    crate::log::begin(cli.command.as_ref().is_some_and(changes));
    if matches!(cli.command, Some(CliCommands::PluginOpen)) {
        if let Err(err) = crate::metadata::start() {
            eprintln!("sidebar metadata: {err}");
        }
        return crate::client::herdr::open_board().map_err(|e| e.to_string());
    }
    if matches!(cli.command, Some(CliCommands::PluginMetadataStart)) {
        return crate::metadata::start();
    }
    if let Some(CliCommands::PluginMetadata { once, cached }) = cli.command {
        return crate::metadata::run(once, cached);
    }
    if let Some(dir) = crate::client::herdr::caller_dir() {
        std::env::set_current_dir(&dir)
            .map_err(|e| format!("can't enter {}: {e}", dir.display()))?;
    }
    let mut projects = load_projects().map_err(|err| err.to_string())?;
    if let Some(CliCommands::Settings {
        default_agent,
        generation_agent,
        generation_model,
    }) = cli.command
    {
        let workflow = if default_agent.is_some()
            || generation_agent.is_some()
            || generation_model.is_some()
        {
            crate::config::update_settings(
                default_agent,
                generation_agent,
                generation_model.as_deref(),
            )?
        } else {
            projects[0].workflow.clone()
        };
        println!(
            "{}",
            serde_json::json!({
                "scope": "global", "agent": workflow.agent,
                "generation_agent": workflow.generation_agent, "generation_model": workflow.generation_model,
                "model_choices": {
                    "claude": crate::client::agent::model_choices(AgentKind::Claude),
                    "codex": crate::client::agent::model_choices(AgentKind::Codex),
                },
            })
        );
        return Ok(());
    }

    if let Some(agent) = cli.agent {
        for project in &mut projects {
            project.workflow.agent = agent;
            project.workflow.agent_override = Some(agent);
        }
    }
    if let Some(CliCommands::Migrate { dry_run }) = cli.command {
        return migrate::run(&projects, dry_run);
    }
    let cwd = crate::tui::caller_dir();
    let command = match cli.command {
        None | Some(CliCommands::Tui {}) => {
            // the TUI opens on the last project it showed, unless told or in one
            let start = match &cli.project {
                Some(name) => Some(choose_project(&projects, Some(name), None)?),
                None => cwd.as_deref().and_then(|d| project_of(&projects, d)),
            };
            return crate::tui::run(projects, start);
        }
        Some(command) => command,
    };
    if let CliCommands::Context { json, task } = command {
        return work::cli_context(&projects, cli.project.as_deref(), task.as_deref(), json);
    }
    let chosen = choose_project(&projects, cli.project.as_deref(), cwd.as_deref())?;
    if let CliCommands::AppSnapshot { cached } = command {
        let _operation = crate::client::herdr::operation();
        return app::snapshot(&projects, chosen, cached);
    }
    let config = projects.swap_remove(chosen);
    let _operation = crate::client::herdr::operation();
    match command {
        CliCommands::Context { .. } => unreachable!(),
        CliCommands::AppSnapshot { .. } => unreachable!(),
        CliCommands::Settings { .. } => unreachable!(),
        CliCommands::Status(args) => table::cli_status(&config, args),
        CliCommands::Start(args) => work::cli_start(&config, args),
        CliCommands::Add(args) => work::cli_add(&config, args),
        CliCommands::Select(args) => work::cli_select(&config, args),
        CliCommands::Note { task, text } => work::cli_note(&config, task, text),
        CliCommands::Rename { args, tab } => work::cli_rename(&config, args, tab),
        CliCommands::Task { command } => work::cli_task(&config, command),
        CliCommands::Prepare { spec } => work::cli_prepare(&spec),
        CliCommands::Attach { key, task } => work::cli_attach(&config, &key, task),
        CliCommands::Review { name } => work::cli_review(&config, name),
        CliCommands::Open { path, shell } => work::cli_open(&config, &path, shell),
        CliCommands::Done(args) => work::cli_done(&config, args),
        CliCommands::Completions { shell } => {
            generate(shell, &mut Cli::command(), "berbr", &mut io::stdout());
            Ok(())
        }
        CliCommands::Tui {} | CliCommands::Logs { .. } | CliCommands::Migrate { .. } => {
            unreachable!()
        }
        CliCommands::PluginOpen => unreachable!(),
        CliCommands::PluginMetadataStart | CliCommands::PluginMetadata { .. } => unreachable!(),
    }
}

/// Whether the command may change tasks, worktrees, tickets or Herdr: those
/// are always logged.
fn changes(command: &CliCommands) -> bool {
    match command {
        CliCommands::Settings {
            default_agent,
            generation_agent,
            generation_model,
        } => default_agent.is_some() || generation_agent.is_some() || generation_model.is_some(),
        CliCommands::Start(_)
        | CliCommands::Add(_)
        | CliCommands::Attach { .. }
        | CliCommands::Select(_)
        | CliCommands::Prepare { .. }
        | CliCommands::Note { .. }
        | CliCommands::Rename { .. }
        | CliCommands::Review { .. }
        | CliCommands::Open { .. } => true,
        CliCommands::Done(args) => !args.dry_run,
        CliCommands::Task { command } => matches!(command, TaskCommand::Update { .. }),
        CliCommands::Migrate { dry_run } => !dry_run,
        CliCommands::Context { .. }
        | CliCommands::AppSnapshot { .. }
        | CliCommands::PluginOpen
        | CliCommands::PluginMetadataStart
        | CliCommands::PluginMetadata { .. }
        | CliCommands::Tui {}
        | CliCommands::Status(_)
        | CliCommands::Logs { .. }
        | CliCommands::Completions { .. } => false,
    }
}

/// Where the Herdr.app mod writes its log: mods may write only there.
fn mod_log() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".config/herdr-app/logs/berbr-mod.log")
}

fn logs(lines: usize, path_only: bool) -> Result<(), String> {
    let logs = [("berbr", crate::log::path()), ("Herdr.app mod", mod_log())];
    for (i, (name, path)) in logs.iter().enumerate() {
        if path_only {
            println!("{}", path.display());
            continue;
        }
        if i > 0 {
            println!();
        }
        println!("== {name}: {}", path.display());
        println!("{}", crate::log::tail(path, lines));
    }
    Ok(())
}

/// The vault's tasks; notes that can't be read are reported and left out.
fn load(config: &Config) -> Result<Vec<Task>, String> {
    let (tasks, skipped) = crate::vault::task::load(config)?;
    for err in skipped {
        eprintln!("skipped {err}");
    }
    Ok(tasks)
}

/// Colors on a terminal only, and never with NO_COLOR set.
fn colors(stream: &impl IsTerminal) -> bool {
    stream.is_terminal() && std::env::var_os("NO_COLOR").is_none()
}

#[cfg(test)]
mod agent_tests {
    use super::*;

    #[test]
    fn agent_can_be_selected_before_or_after_subcommand() {
        for args in [
            vec!["berbr", "--agent", "codex", "start", "task"],
            vec!["berbr", "start", "task", "--agent", "codex"],
            vec!["berbr", "review", "--agent", "codex"],
        ] {
            assert_eq!(
                Cli::try_parse_from(args).unwrap().agent,
                Some(AgentKind::Codex)
            );
        }
        assert_eq!(
            Cli::try_parse_from(["berbr", "--agent", "claude"])
                .unwrap()
                .agent,
            Some(AgentKind::Claude)
        );
        assert!(Cli::try_parse_from(["berbr", "--agent", "other"]).is_err());
    }
}
