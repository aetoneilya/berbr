# Berbr for Herdr App

Native GPUI mod in the right panel, alongside Files and Changes. The panel
has one search row with a **Filters** button. Its popover chooses **Tasks / MR**,
one or more projects, and whether completed work is shown. The selected project
scope is saved. Project snapshots load independently, at most two at a time;
failures retain cached rows and identify the affected project. Rows are labelled
with their project, and actions keep the source project's context even when task
IDs coincide. New task stays below the list; its form selects the creation project.
Refresh and Settings are in the footer.
Tasks shows task status and agents needing
attention. Click a task to select its Herdr space; the separate `⋯` button
opens its note and related tickets, merge requests, worktrees and agents.
Tasks without a live space open details with an explicit Start work action.

MR shows a flat list of your GitLab merge requests and GitHub pull requests across the selected projects' configured
repositories, including MRs without a task or local checkout. Each MR appears once,
with its repository, associated tasks, CI, approvals, unresolved discussions,
draft/conflict and merge readiness. Click or press Enter to open it in the browser;
right-click offers the usual link/branch actions. Search matches titles, branches,
repositories and task names. The last Tasks / MR choice is saved for each project;
switching views keeps their searches and selections separate.

Only open MRs appear initially. **Show completed** in Filters (or `d`) includes merged and closed
MRs already present in the current snapshot, rather than loading a full history.
The additive `merge_requests` snapshot field contains every fetched MR with its
full title, repository, ticket keys and task associations, independently of tree
presentation. Older backends fall back to the MRs present in the task tree.
Unavailable CI is shown as unknown. Cached results and source warnings follow the
same refresh behavior as Tasks. `berbr.tasks` and `berbr.mrs` can also be bound as
Herdr commands. GitHub PRs use the configured GitHub integration.

Expanded tasks also show **Artifacts**, a shared tree of any files and folders.
Folders load on expansion; only visible directories refresh every five seconds.
The header can refresh them manually or open the root folder in Finder.
Markdown uses Berbr's note viewer/editor, other files their default app, and
scripts/executables the text editor. Symlinks are revealed rather than traversed.
Artifacts live under Berbr's home, outside worktrees and task-note directories,
and remain available for closed tasks.

The existing Rust model owns Jira, GitLab, notes, worktrees and Claude/Codex
history. The mod consumes `berbr app-snapshot` JSON. It launches the backend through
login/interactive zsh to inherit the same credentials as the terminal TUI;
arguments are passed literally, without interpolating them into shell code. Snapshots never update
task notes; cached startup is followed by an online refresh. Local refreshes
run every 30 seconds, online ones every five minutes. Errors and cached
service results are shown in the panel. Live agent attention comes from the
app. Remote machines are excluded from this first local-project adapter.

## Install

```sh
cargo build --release --locked
python3 scripts/install-herdr-app.py --activate
```

The installer makes a personal `berbr` layout from the current layout, preserving
the original. It copies the backend binary into the mod's permanent folder.
`tabs-berbr` is a personal copy of the shipped tab bar with one additional
panel button. Shipped files and keybindings are untouched. Reinstalling backs
up previous mod versions and does not replace the existing personal layout.
The install expects standard `tabs`, `panel`, and `panel-body` layout nodes.

Switch back with `herdr-app do layout.switch '{"name":"<previous-layout>"}'`.

The terminal Herdr backend and TUI remain available. To use the TUI or CLI
with the desktop backend, set `BERBR_BACKEND=herdr-app` explicitly. The app
adapter uses `herdr-app` for topology/focus and `hc` for terminals. It checks
that a newly created terminal has an available POSIX shell before submitting
a launch command. Codex retains `--no-daemon` in every launch path.

## Validation

```sh
cargo test --locked
herdr-app check herdr-app/berbr
herdr-app check herdr-app/tabs-berbr
```

Contract tests cover backend failure visibility, read-only snapshots,
excluding remote machines, retaining Codex arguments, restoring caller focus,
and refusing to submit commands to an occupied terminal. Visual checks must
also open the panel, task details and the New task form in the app; the static
mod check does not exercise loaded data or deferred forms.
