# Recovering from Berbr workflow errors

Read only the section relevant to the current failure. Installed command help
is authoritative; a missing capability should be reported, not guessed.

## Task lookup: no task here / no task ID / ambiguous checkout

Check the project and stable ID in the supplied brief. From another checkout,
use both explicit selectors:

```sh
berbr -p "<project>" task show "<task-id>"
berbr -p "<project>" note --task "<task-id>" 'A verified finding'
```

Use `berbr context --json` first; it returns candidates instead of guessing on
ambiguity. Select with `-p` and `--task` as needed. If no ID was supplied, list
local tasks with `berbr -p "<project>" task list --json` (add `--all` for closed
tasks). Its `tasks[].id` is the stable task ID. On an older CLI, use
`app-snapshot --cached`: for rows with `kind: "task"`, use `task`, not UI row `id`.
A checkout/branch label is not a task ID. A task can
have multiple checkouts and an unrelated human title. Do not retry with guessed
names or create a replacement task just to append a finding.

If an explicit project/ID still cannot be found, inspect configuration selection:
`BERBR_CONFIG` wins over legacy `WORKTABLE_CONFIG`, then the configured plugin
or default config is used. Do not dump config contents or credentials into a
report. A legacy override can point at a different task store; verify the
intended config before changing an inherited override. Conflicting explicit
worktree bindings need an explicit task; do not silently choose one.

## Batch add / missing KEY / surprising checkout name

Pass one argv value per argument and quote runtime values. In zsh, `set -- $spec`
does not perform bash-style splitting: a string such as `video DEMO-12 15` may
remain one argument, leaving `$2` empty. Prefer explicit calls or a small function:

```sh
add_ticket() {
  berbr add "$1" --name "$2" --description "$3" --bg
}
add_ticket DEMO-12 video-cleanup 'Inspect the video cleanup brief'
add_ticket DEMO-13 feed-cleanup 'Inspect the feed cleanup brief'
```

For generated batches, a programmatic subprocess argv list avoids shell splitting.
Check each return code and keep the full failure output. `... | tail -1` can
hide the failed command's status and discard launch information. On partial
success, inspect the returned checkout/terminal before retrying; do not duplicate
successful launches. `--in` shares an existing branch; it is not a substitute
for creating a new branch.

For a new branch without a ticket, use `add --name <slug>` in the current task,
or `add <task-id> --name <slug>` with the project selected explicitly. A failure
to record the binding after checkout creation reports the remaining path; do not
create another task or blindly retry a successful earlier step.
A reserved name can belong to a still-pending `start` checkout. Inspect that
launch or choose another name; do not delete its reservation to force `add`.

## Operation not permitted / permission denied

Task notes and Berbr's shared write lock may live outside the current worktree.
New Codex launches receive task-directory/cache grants, but an already running
agent or its controlling harness may still have narrower permissions.

First use `task show`. If the required title and description already exist,
there is no need to issue a startup update. Current `task update --fill-empty`
also skips note writes and the write lock when no supplied field changes.
Actual changes still need the task directory (atomic replacement) and shared
cache lock. Use the harness's normal mechanism to request the narrowly scoped
access needed for that operation, or report the blocked write if it is unavailable.
Do not hand-edit a copy, disable the sandbox or change filesystem permissions.
Continue authorized work that does not depend on the blocked operation.

## Naming agent failed / snapshot returned a negative code

Use `berbr logs` locally and keep the error, command, time and selected project.
Logs can contain task descriptions and findings: share only relevant, sanitized
excerpts. Naming uses its own CLI/model settings, independently of the work
agent; inspect `berbr settings` before blaming the selected work agent. A failed
name generation does not create a task or worktree. An explicit title and
`--name` can skip generation when they fit the user's request.

A negative result without stdout/stderr does not establish a timeout or signal.
Report the observed status and missing diagnostics. Do not repeat a mutation
blindly: inspect current tasks/worktrees/terminals first, since the process may
have completed some steps before it stopped.

## Helper launch did not deliver / wait returned early

Follow `hc --skill` for the installed version. Check `first_message.sent` when
available, then `hc get`/`hc read` for readiness or a trust/permission dialog.
Read before resending; resolve the actual gate. A wait timeout means the helper
may still be working. A completed turn requires collecting the actual result.
For sandbox failures accessing the core socket, use the harness's normal access
mechanism; do not launch a second core or restart the user's terminals.
