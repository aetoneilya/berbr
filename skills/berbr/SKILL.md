---
name: berbr
description: Work in a Berbr task using its task note, worktrees and visible Herdr agents. Use for Berbr task context, findings, additional worktrees or agents, and recovery from Berbr CLI errors.
---

# Working in a Berbr task

A task has a stable ID, a note, a Herdr space, worktrees and agents. The default
checkout root is `.herdr/worktrees`; configuration can change it, and older
checkouts may use `.claude/worktrees`. Keep work in the assigned checkout so
Berbr can associate the session with its task. A title, branch name and task ID
are different identifiers.

## Find the context and preserve it

Read the supplied task note and brief first. The brief names the checkout and
branch; its `task.md` lives in the checkout's Git directory, outside tracked
files. Repository instructions and the user's existing authorization still apply.

Use `berbr context --json` to resolve project, task, checkout, branch and brief
from local data. Check `status`: only `resolved` has a selected `task`. Other
statuses include candidates and require an explicit selector; inspection still
exits 0. It does not guess the first configured project from an unrelated folder.
Use `berbr -p "<project>" context --task "<task-id>" --json` to choose explicitly.

`berbr task show` returns the task's full JSON with `id`, `title`,
`description`, `note` and `done`. It does not return the worktree inventory.
If resolution fails or you operate from another checkout, use the project and
ID supplied in the brief:

```sh
berbr -p "<project>" task show "<task-id>"
berbr -p "<project>" note --task "<task-id>" 'A verified finding'
```

For an overview, use `berbr -p "<project>" app-snapshot --cached` (JSON) or
`berbr -p "<project>" status --no-write`. Plain `status` updates task notes.
When the ID is unknown, use `berbr -p "<project>" task list --json`; each item
under `tasks` has its stable `id`. Add `--all` to include closed tasks. If the
installed CLI predates these commands, snapshot rows with `kind: "task"` expose
the task ID in `task`; their row `id` is a UI identifier such as `task:demo-0`.
Do not guess an ID from the branch.
An explicit task still belongs to the selected project: `--task` does not select
a project. Read the full `berbr <command> --help`, including Options, when unsure.

Record decisions, verified findings, dead ends and handoffs with `berbr note`.
It appends under Agent notes without replacing requirements. The task ID is an
option (`note --task ID TEXT`), not a positional argument before the finding.
Do not edit task notes by hand.

After a substantive user request, inspect the task. Fill only missing title or
description fields if the brief calls for it; populated fields need no update.
Preserve user text and other agents' findings. For an authorized description
change, `task update --description-file <file>` or `--description-file -` accepts
multiline UTF-8. `--fill-empty` preserves existing fields; omitting it replaces
the supplied fields. Use `berbr rename` for an authorized title/space rename;
do not rename files, branches or worktrees yourself to match a new title.

## Choose the right kind of additional work

Only start an agent or a separate task when the assignment calls for it.

- **Another branch for a ticket in this task:**
  `berbr add "<KEY>" --name "<short-slug>" --description '<assignment>' --bg`.
  It creates a checkout, branch, tab and agent in the current task. Use an
  explicit task positional argument when working outside its context; use `-p`
  for a different project. An existing ticket needs `--name` for another branch.
- **Another branch in the same task without a ticket:**
  `berbr add --name "<short-slug>" --description '<assignment>' --bg`.
  From outside the task, pass its ID: `berbr -p "<project>" add "<task-id>" --name
  "<short-slug>" --description '<assignment>' --bg`. Berbr saves the checkout's
  task binding; no invented ticket or separate task is needed.
- **Another agent in an existing task checkout:**
  `berbr add --in "<checkout-name-or-full-path>" --description '<assignment>' --bg`.
  This shares the existing branch; it does not create a second or stacked MR.
  Coordinate writers. Full paths disambiguate repeated checkout names.
- **A separate task requested by the user:**
  `berbr start "<title>" --name "<short-slug>" --description '<assignment>' --bg`.
  Explicit `--name` gives a predictable checkout name. Otherwise it may be
  derived from the title, ticket or generated description, falling back to an ID.

`--description` starts work immediately. `--message` supplies context but leaves
the agent waiting. Report the returned checkout and terminal address. Continue
branch work through the agent assigned to that checkout; do not edit another
checkout from the parent with `git -C`. Let Berbr create task worktrees rather
than making an unregistered checkout manually.

KEY is optional with `--name` or `--in`. On an older binary that still requires
KEY for a new checkout, report the version limitation rather than inventing a
key or silently splitting the work into a separate task.

## Visible helpers in the same checkout

For an assigned review or investigation that does not need a new branch, use
an interactive `hc` terminal in the task checkout. Before launching, read
`hc --skill` and check `hc templates` for a suitable role. A direct example:

```sh
hc run --bg --name review --cwd "$PWD" codex --no-daemon '<scoped assignment; no edits or child agents>'
hc wait review --timeout 45
hc read review --lines 100
```

Choose sandbox and approval settings appropriate to the assigned work and the
current harness; do not copy permissive settings from another session. Codex
needs `--no-daemon` so its commands run inside the visible terminal. Claude can
be launched interactively instead of Codex. Put the assignment in the initial
launch, rather than sending it to a booting agent.

Use the returned name/address for `hc wait`, `get`, `read` and `send`. A timeout,
startup idle or notification is not a completed assignment: inspect the reported
status and actual result. Read before resending a possibly delivered assignment.
For `blocked`, inspect the question or permission request and honor that gate.
Clean up only helpers you started, after collecting their result, with `hc kill`
then `hc rm`. Do not restart or stop the user's core to recover one helper.

## Recovery and task boundaries

For task lookup errors, shell argument failures, sandbox denials, or an agent
that did not start, read [references/recovery.md](references/recovery.md).

Closing tasks, removing worktrees or transitioning tickets requires user
authorization; an implementation request alone does not authorize cleanup.
Treat a refusal as information, not an instruction to retry with `--force`.
Use the current command's help and inspection/dry-run mode before an authorized
destructive action. A skill never grants commit, push, publish or production
access beyond the user's instructions.
