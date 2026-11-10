# Local agent CLI contract

These commands read the selected configuration and local task notes. They do
not fetch Jira/GitLab, query live Herdr, scan agent transcripts or save caches.
Logging may still write when the user has explicitly enabled debug logging.
Invalid arguments/configuration remain ordinary CLI errors with nonzero exit.

## context

```sh
berbr context --json
berbr -p demo context --task demo-0 --json
```

JSON schema version 1 contains:

| Field | Meaning |
|---|---|
| `status` | `resolved`, `unbound`, `ambiguous`, `no_project` or `ambiguous_project` |
| `project`, `project_candidates` | Selected project name or null, plus candidate names |
| `cwd`, `repo`, `worktree`, `branch` | Actual local directory/Git context; absent Git facts are null |
| `in_project` | Whether the current Git checkout belongs to a configured repository in the selected project |
| `task` | Selected `{id,title,note,done}` object, or null |
| `brief` | Existing brief path for the selected task in the current checkout, or null |
| `source` | `explicit`, `binding`, `stored_reference`, `saved_workspace`, `none`, or null when no project is selected |
| `candidates` | Task objects to help choose an ID; unresolved contexts never select one merely because only one task exists |
| `warnings` | Skipped malformed notes, unavailable configured repositories, or lookup ambiguity |

Inspection returns exit 0 even when unresolved. Consumers must check `status`
and `task`, rather than interpreting exit 0 as proof of an association.

Project selection honors `-p`, then Git common-directory identity (including
external linked checkouts), then the nearest containing configured repo path.
Multiple equally applicable projects remain ambiguous; unrelated directories
return `no_project` instead of defaulting to the first configured project.

Within a project, `--task` explicitly chooses an ID/title. Otherwise absolute
`worktrees`/legacy `bindings.worktrees` references win over basename references.
A basename reused across repositories is ambiguous. Stored task session paths,
stable task-ID checkout names and branch ticket keys can infer ownership when
no explicit binding matches. A saved `space` ID can resolve the current
`HERDR_WORKSPACE_ID` when no checkout match exists. Closed tasks are not inferred,
but can be selected explicitly. Conflicting bindings or references return task
candidates; live workspace labels and unsaved session history are not queried.

The Git fields always describe the actual current checkout, even when `-p` and
`--task` select a task elsewhere. `brief` remains null if ownership of the current
checkout is not established; it must not point at a different task's instructions.
Missing brief files also produce null. Human-readable output is the default
when `--json` is omitted.

## task list

```sh
berbr -p demo task list --json
berbr -p demo task list --all --json
```

JSON is `{version:1,project,tasks,warnings}`. Each task includes `id`, nullable
`title`, `note`, `done`, `tickets` and `tags`; description/history are omitted.
Open tasks are listed by default; `--all` includes closed tasks. Entries sort by
stable ID. Malformed notes are skipped with warnings rather than silently
pretending the list is complete. This command follows the standard project
selection rules (`-p`, current directory, then the first configured project).
Use an explicit project when operating outside a known checkout.

## add without a ticket

```sh
berbr add --name parser-followup --description 'Inspect parser ordering' --bg
berbr -p demo add demo-0 --name parser-followup --description 'Inspect parser ordering' --bg
```

`--name` makes KEY optional for a new checkout; `--in` makes it optional for an
existing checkout. The one-positional-argument form still treats a ticket-shaped
argument as KEY; use the current task context for a legacy task named like a
ticket. A new ticketless checkout uses the configured branch prefix plus name,
and its absolute path is appended to the task's `worktrees` under the note-write
lock. Existing tickets, requirements and findings are preserved.

The command still requires Herdr for its agent tab, preserves noninteractive
background behavior and refuses checkout collisions. It atomically claims the
same name-reservation file used by asynchronous `start`/`select`: a name reserved
for a pending checkout is refused even before that directory exists. Explicit
`add --name` does not silently choose a suffix or steal another task's reservation.
`--description` starts the
assignment; `--message` leaves the agent waiting. A failure after checkout creation
reports the remaining path and any known tab/terminal IDs; task-note failure does not launch the agent. Inspect
partial results before retrying rather than inventing a ticket or another task.
