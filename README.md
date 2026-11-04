# berbr

berbr joins Jira tickets, GitLab merge requests, git worktrees,
[herdr](https://herdr.dev) agents and task notes in an Obsidian vault into one
table. A task is a markdown note; its tickets, MRs, worktrees, agents and claude
sessions are found from branch names, herdr workspaces and transcripts. `berbr`
shows the table as a TUI or as plain text and runs the daily workflow: start a
task, add a ticket to it, go back to it, review, close. `DESIGN.md` has the
model and the decisions.

## Install

Repository: [example/berbr](https://github.com/example/berbr).

```
cargo install --path . --locked
```

## Config

`~/.config/berbr/config.toml` (`$XDG_CONFIG_HOME/berbr/config.toml`, or
the path in `BERBR_CONFIG`):

```toml
[vault]
path = "~/demo-vault"

[workflow]                  # defaults for every project
branch_prefix = "demo/"
# agent = "codex"           # default: "claude"; also accepts --agent per command
# review_model = "claude-fable-5-1"
# ide = "IntelliJ IDEA"

[[project]]                 # in the TUI's project dropdown, `berbr -p demoapp`
name = "demoapp"
task_dirs = ["30-work/tasks"]
# new_task_dir = "30-work/tasks"   # default: the first of task_dirs
jira = { host = "https://jira.example.com", project = "DEMO", issue_type = "Задача" }
gitlab = { host = "gitlab.example.com", project = "demo/service" }

[[project.repo]]
path = "~/projects/demoapp"
gitlab_project = "demo/service"

[[project]]                 # no Jira, no GitLab: notes, worktrees, agents
name = "vpn"
task_dirs = ["10-ah-fuck-life/projects/vpn"]
branch_prefix = ""

[[project.repo]]
path = "~/code/pet-projects/vpn-bot"
```

Existing `~/.config/worktable/config.toml` is used when the new config is absent.
`WORKTABLE_CONFIG`, `WT_TRACE` and `WT_POPUP` remain accepted; the `BERBR_*`
variables take precedence. An existing `worktable` cache is reused, including its
note-write lock, so old and new binaries can safely share task notes. New
installations use the `berbr` cache directory.

Commands work on one project: `-p <name>`, else the one whose repo the current
directory is in, else the first. The TUI opens on the last one shown; ↑ from the
top of the list gets to the project dropdown, ⏎ opens it, and its "+ new project"
appends a `[[project]]` here. `[` `]` and Tab switch at once.

Tokens come from `JIRA_TOKEN` and `GITLAB_TOKEN`. `BERBR_TRACE=1` prints how long
each fetch, the matching and the rendering took to stderr.

## Commands

Choose Codex with `berbr --agent codex start <task>` or `[workflow] agent =
"codex"`. Codex always runs with `--no-daemon`, including new sessions, reviews,
resumes and launches after worktree preparation, so each pane owns its runtime.

```
berbr                         the TUI (also `berbr tui`)
berbr start [<task|KEY>]      new task: note, herdr workspace, worktree tab with claude
berbr add [<task>] <KEY>      add a ticket to a task, with its own worktree tab
berbr attach <KEY> [<task>]   put a ticket into a task without a worktree
berbr select [<task|KEY>]     go back to a task: its agent, or claude resuming its session; a task with nothing yet gets its first worktree
berbr note "<text>"           (for agents) append a finding to the note's "Agent notes"
berbr status                  the table in plain text; updates `state` and `sessions` in notes
berbr review [<worktree>]     start a reviewer agent next to the worktree's agent
berbr done [<task|worktree>] [--merged] [--force] [-y]
                           close a task (worktrees, my Jira tickets, note marked
                           done, its herdr workspace),
                           or remove one worktree; --merged removes every finished one
berbr completions <shell>     shell completion script
```

`berbr <command> --help` lists the options.

## TUI

`?` shows all keys. The main ones:

- `↑↓`/`jk` move, `←→`/`hl`/space fold, `z` fold / unfold all, `/` filter, `.` all / active only, `v` hide MRs, worktrees, …
- `⏎` focus the agent, or resume work (`berbr select`)
- `+ new task` row above the tasks: ⏎ or a click opens a form (name, one Jira dropdown: none, new ticket and the open tickets without a task, filtered by typing; checkboxes worktree, agent); `s` start a ticket / add one to a task, `a` attach a ticket
- `b` browser, `o` note in Obsidian, `e` note in `$EDITOR`, `i` worktree in the IDE
- `R` review, `d` remove a worktree / close a task
- `w` next agent waiting for you, `p` details pane, `r` refresh, `q` quit

## herdr popup

In herdr's config, `prefix+t` opens the TUI as a popup; going to some work
closes it:

```toml
[[keys.command]]
key = "prefix+t"
type = "popup"
command = "zsh -ic 'BERBR_POPUP=1 exec berbr'"
```

## Herdr App

The desktop version has a native task list in the right panel, alongside Files
and Changes. See [Herdr App installation and behavior](herdr-app/README.md).
