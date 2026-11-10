# Berbr for Herdr App

Native GPUI mod in the right panel, alongside Files and Changes. The panel
shows tasks only: project, search, creation, task status and agents needing
attention. Click a task to select its Herdr space; the separate `⋯` button
opens its note and related tickets, merge requests, worktrees and agents.
Tasks without a live space open details with an explicit Start work action.

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
