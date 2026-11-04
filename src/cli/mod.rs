use crate::client::agent::AgentKind;
mod app;
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
    /// Read-only JSON board for the native Herdr App mod.
    AppSnapshot {
        /// Read cached Jira/GitLab data without network calls.
        #[arg(long)]
        cached: bool,
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
    /// Add a ticket to a task, with its own worktree tab in the task's workspace
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
    /// (for agents) Append a finding to the task note's "Agent notes"
    Note {
        /// Explicit task ID or title; defaults to the current worktree/workspace
        #[arg(long)]
        task: Option<String>,
        #[arg(trailing_var_arg = true, required = true)]
        text: Vec<String>,
    },
    /// Read or update the current task's title and description (for agents)
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    /// Tasks with their tickets, merge requests, worktrees and agents; then work
    /// that belongs to no task. Updates `state` and `sessions` in task notes.
    Status(StatusArgs),
    /// Start a reviewer agent next to the task's agent
    Review { name: Option<String> },
    /// Close a task: `berbr done "<task>"` (or a ticket of it) removes its finished
    /// worktrees, closes my tickets in Jira and marks the note done. With a
    /// worktree name, or none inside one: remove that worktree
    Done(DoneArgs),
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
    /// Worktree, tab and agent name; default: ticket summary, or task ID
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
    /// [<task>] <KEY>; the task defaults to the current one
    #[arg(num_args = 1..=2, required = true)]
    pub(crate) args: Vec<String>,
    /// Run the agent in this existing worktree of the task (stacked MRs)
    #[arg(long = "in")]
    pub(crate) in_tree: Option<String>,
    #[arg(long, short = 'n')]
    pub(crate) name: Option<String>,
    #[arg(long, short = 'm')]
    pub(crate) message: Option<String>,
    /// A description for the agent to work on immediately
    #[arg(long)]
    pub(crate) description: Option<String>,
    #[arg(long)]
    pub(crate) bg: bool,
}

#[derive(Args, Debug, Default)]
pub(crate) struct SelectArgs {
    /// Task name or ticket key; a picker when omitted
    pub(crate) target: Option<String>,
    /// That worktree of the task; for a task without any, the name of its
    /// first one
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
    name: Option<String>,
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
    if let Some(agent) = cli.agent {
        for project in &mut projects {
            project.workflow.agent = agent;
            project.workflow.agent_override = Some(agent);
        }
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
    let chosen = choose_project(&projects, cli.project.as_deref(), cwd.as_deref())?;
    if let CliCommands::AppSnapshot { cached } = command {
        return app::snapshot(&projects, chosen, cached);
    }
    let config = projects.swap_remove(chosen);
    match command {
        CliCommands::AppSnapshot { .. } => unreachable!(),
        CliCommands::Status(args) => table::cli_status(&config, args),
        CliCommands::Start(args) => work::cli_start(&config, args),
        CliCommands::Add(args) => work::cli_add(&config, args),
        CliCommands::Select(args) => work::cli_select(&config, args),
        CliCommands::Note { task, text } => work::cli_note(&config, task, text),
        CliCommands::Task { command } => work::cli_task(&config, command),
        CliCommands::Prepare { spec } => work::cli_prepare(&spec),
        CliCommands::Attach { key, task } => work::cli_attach(&config, &key, task),
        CliCommands::Review { name } => work::cli_review(&config, name),
        CliCommands::Done(args) => work::cli_done(&config, args),
        CliCommands::Completions { shell } => {
            generate(shell, &mut Cli::command(), "berbr", &mut io::stdout());
            Ok(())
        }
        CliCommands::Tui {} => unreachable!(),
        CliCommands::PluginOpen => unreachable!(),
        CliCommands::PluginMetadataStart | CliCommands::PluginMetadata { .. } => unreachable!(),
    }
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
