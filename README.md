# berbr

berbr joins GitHub Issues and Pull Requests, Jira tickets, GitLab merge requests, git worktrees,
[herdr](https://herdr.dev) agents and task notes into one table. A task is a markdown note; its tickets, MRs, worktrees, agents and claude
sessions are found from branch names, herdr workspaces and transcripts. `berbr`
shows the table as a TUI or as plain text and runs the daily workflow: start a
task, add a ticket to it, go back to it, review, close. `DESIGN.md` has the
model and the decisions.

## Install


```
cargo install --path . --locked
```

## Where things live

Like `~/.claude`, berbr keeps its own folder, `~/.berbr` (`BERBR_HOME` overrides):

```
~/.berbr/
  config.toml
  logs/berbr.log
  projects/<project>/tasks/*.md        open tasks
  projects/<project>/tasks/done/*.md   closed ones, moved there by `berbr done`
```

Task notes are plain Markdown with YAML frontmatter; `o` in the TUI and the
mod's note window open them in the default editor.

An Obsidian vault can still hold them: a `[vault]` table puts every project's
`task_dirs` inside it, as before. `berbr migrate` moves from there to `~/.berbr`:
it copies each task (closed ones into `done/`, with its creation date), the
task-number counters and the config without `[vault]` to
`~/.berbr/config.toml`. The vault's notes are left alone — delete them once the
copy looks right; removing `~/.berbr/config.toml` goes back.

## Config

The first of `BERBR_CONFIG`, `~/.berbr/config.toml`,
`$XDG_CONFIG_HOME/berbr/config.toml`:

```toml
[workflow]                  # defaults for every project
branch_prefix = "demo/"
worktree_dir = ".herdr/worktrees" # relative to each main checkout
# agent = "codex"           # default: "claude"; also accepts --agent per command
# generation_agent = "claude" # independent CLI for naming tasks from Description
# generation_model = ""      # empty: the naming CLI's configured default
# review_model = "claude-fable-5-1"
# ide = "IntelliJ IDEA"

[[project]]                 # in the TUI's project dropdown, `berbr -p demo`
name = "demo"               # tasks in ~/.berbr/projects/demo/tasks
jira = { host = "https://jira.example.com", project = "DEMO", issue_type = "Задача" }
gitlab = { host = "gitlab.example.com", project = "demo/service" }

[[project.repo]]
path = "~/projects/demo"
gitlab_project = "demo/service"

[[project]]                 # no Jira, no GitLab: notes, worktrees, agents
name = "vpn"
branch_prefix = ""

[[project.repo]]
path = "~/code/pet-projects/vpn-bot"
```

With a vault instead, add `[vault] path = "~/demo-vault"` and a `task_dirs =
["30-work/tasks"]` (relative to the vault) to each project; `new_task_dir`
defaults to the first of them.

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

### GitHub.com

GitHub works without Jira or GitLab. Configure the repository whose issues and
PRs you want to see, and its local checkout:

```toml
[[project]]
name = "demo"
github = { repo = "example/demo" }

[[project.repo]]
path = "~/code/demo"
```

Authenticate with `gh auth login --hostname github.com` (Berbr reads its saved
token), or set `GH_TOKEN` / `GITHUB_TOKEN`. Precedence is `GH_TOKEN`, then
`GITHUB_TOKEN`, then `gh auth token --hostname github.com`. The token must have
access to the configured repositories. A classic token needs `repo` for private
repositories; a fine-grained token needs read access to repository metadata,
Issues, Pull requests, Commit statuses and Checks. Organization SSO may also
need authorization. Berbr only reads GitHub; it never creates/closes issues or
publishes/merges PRs. GitHub Enterprise is outside this release.

The CLI, TUI's new-task ticket picker, and Herdr's **GitHub** tab list open
issues, including unassigned ones. In the CLI, use:

```sh
berbr start example/demo#42
berbr start https://github.com/example/demo/issues/42
berbr attach example/demo#42 demo-0
berbr add demo-0 example/demo#42 --name followup
berbr select example/demo#42
```

Quote `'#42'` to use a number when the project has exactly one configured GitHub
repository. With multiple repositories use `owner/repo#42`. Task notes store
`github.com/owner/repo#42`, so equal issue numbers in different repositories are
distinct. GitHub issue branches use `issue-42-<name>`; the saved checkout path
keeps the task binding. The agent's brief includes the issue title and body.

For several checkouts, set `github_repo = "owner/repo"` in each
`[[project.repo]]`. A repository can select GitHub or GitLab; a project may use
both, alongside Jira. An explicit repository setting overrides the project
default. To work in a fork, keep the upstream in `github.repo` and put the fork
in the checkout's `github_repo`. Berbr also discovers a fork's upstream from
GitHub repository metadata. PRs match the source repository **and** branch,
while PR numbers and issue references belong to the target repository. Only
API-confirmed issues from configured repositories are added to task notes;
references to other PRs or unconfigured upstream issues are excluded. Jira keys
in GitHub branches and PRs keep their existing task-matching behavior.

PR discovery includes open PRs and recently updated merged/closed PRs (90 days),
plus older PRs for local checkout branches. Tasks without issues match their PRs
through their saved worktrees. The panel shows draft, checks, approvals, review
decision, unresolved threads and conflicts, with GitHub links and PR labels.
Ready requires complete current review data, successful checks for the current
head, a clean merge verdict, and no draft, conflict or unresolved thread.
Unknown checks, changed heads, incomplete reviews and cached data never show
ready. Missing credentials, access failures, rate limits and network failures
show a warning and reuse the last successful cache for the configured set of
repositories. `app-snapshot --cached` makes no remote requests.

Snapshot v3 retains `kind: "mr"` for compatibility. GitHub rows add
`provider: "github"`, qualified `repository` / `head_repository`, and
`review_decision`; open PRs use `state: "open"` and labels `#42`. The `sources`
object adds `github`. Closing a Berbr task leaves its GitHub issues open.

Commands that change something (start, select, done, rename, …) are logged with
their arguments, folder, time and error to `~/.berbr/logs/berbr.log`
(`BERBR_LOG_DIR` overrides); the Herdr.app mod
logs what it ran to `~/.config/herdr-app/logs/berbr-mod.log`. `debug = true` at
the top of the config, or `BERBR_DEBUG=1`, also logs reads, every git and herdr
call, each HTTP request (method, URL without query, status) and the trace spans.
`berbr logs` shows the end of both.

## Commands

Choose Codex with `berbr --agent codex start <task>` or `[workflow] agent =
"codex"`. Codex always runs with `--no-daemon`, including new sessions, reviews,
resumes and launches after worktree preparation, so each pane owns its runtime.

```
berbr                         the TUI (also `berbr tui`)
berbr start [<task|KEY>]      new task: note, herdr workspace, worktree tab with claude
berbr add [<task>] [<KEY>]    new agent tab; KEY is optional only with --in <tree>
berbr attach <KEY> [<task>]   put a ticket into a task without a worktree
berbr select [<task|KEY>]     go back to a task: its agent, or claude resuming its session; a task with nothing yet gets its first worktree
berbr note "<text>"           (for agents) append a finding to the note's "Agent notes"
berbr status                  the table in plain text; updates `state` and `sessions` in notes
berbr review [<worktree>]     start a reviewer agent next to the worktree's agent
berbr done [<task>] [--tree <name-or-path> [--repo <path>]] [--merged] [-y] [--dry-run [--json]]
                           close a task (worktrees, my Jira tickets, note marked
                           done, its herdr workspace),
                           or remove one worktree; --merged removes every finished one
berbr completions <shell>     shell completion script
```

For `note` and `task show/update`, an explicit `worktrees` path (including legacy
`bindings.worktrees`) takes precedence over inferred sessions and workspace.
Conflicting explicit bindings require `note --task <id>` or `task show/update <id>`;
use `-p <project>` when the current directory belongs to another project.
Absolute paths take precedence over basenames; a basename shared by several
repositories is ambiguous. These rules do not change table ownership or task
closing plans.

Task agents started or resumed with Codex receive `--add-dir` for the task note's
directory and Berbr's cache (the shared note-write lock). Existing running agents
keep their current sandbox permissions until restarted or granted access.
An inherited `WORKTABLE_CONFIG` still selects that explicit configuration;
after migration, remove the stale override or point it to `~/.berbr/config.toml`.

`done --dry-run` prints the same plan without changing Jira, notes, worktrees,
branches, herdr or caches. Add `--json` for one plan object, including blockers,
removals, kept trees and ticket actions. Blockers still exit 0 in dry-run; the
listed steps are conditional on resolving them. Without dry-run they refuse the
operation. A task ticket alias still selects its task. To remove a checkout,
use `berbr done --tree <name-or-path> [--repo <main-checkout>]`: this target never
resolves a task or ticket. Positional arguments select tasks only. Duplicate
worktree names require a full checkout path or
`--repo`. The mod uses this explicit target too. `--merged` sweeps clean,
finished checkouts.

Task closing lists each dirty checkout and its uncommitted file count before
confirmation; JSON includes both `dirty` and the counts in `remove`. Accepting
that plan discards those files. Single-worktree removal refuses dirty checkouts.
A failed removal or Jira transition reports the failed step and leaves the note
and Herdr space open for retry; successful prior steps remain completed.
Unknown inspection counts are JSON `null`.

`berbr <command> --help` lists the options.

## TUI

`?` shows all keys. The main ones:

- `↑↓`/`jk` move, `←→`/`hl`/space fold, `z` fold / unfold all, `/` filter, `.` all / active only, `v` hide MRs, worktrees, …
- `⏎` focus the agent, or resume work (`berbr select`)
- `+ new task` row above the tasks: ⏎ or a click opens a form (name, one Jira dropdown: none, new ticket and the open tickets without a task, filtered by typing; checkboxes worktree, agent); `s` start a ticket / add one to a task, `a` attach a ticket
- `b` browser, `o` note in its editor (Obsidian for a vault), `e` note in `$EDITOR`, `i` worktree in the IDE
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

The small **Settings** button at the bottom of the panel sets the default work
agent. **New task → Generate → Task naming** expands the agent and model used to
name the task and branch from Description. Choices are saved when creating a task;
opening the section or cancelling does not change settings. Naming and working
agents are independent. Models come from Claude CLI aliases or Codex's visible
local model catalog; **CLI default** is always available.

These defaults apply to every project and are stored in `[workflow]`, preserving
comments and project data. Legacy project-level agent and naming overrides are
ignored; existing conversations retain their owning CLI. They are also available
through `berbr settings`, `berbr settings --default-agent codex`, and
`berbr settings --task-naming-agent claude --task-naming-model haiku`. An explicit
task name skips naming; a naming failure leaves the task uncreated.

`berbr app-snapshot [--cached]` emits snapshot version 3, retaining all v1/v2 fields.
Task `body` is a human preview capped at 2 KiB of UTF-8, before fenced-code-aware
Agent notes / Sessions / Сессии sections. `body_truncated` marks omitted text.
The mod searches previews and reads the full file on demand for Note viewing and
editing. Older mods can still read `body`; newer mods also accept older full-body
snapshots. MR and worktree `body` fields keep their existing meanings.
Worktree rows retain `merged` and `mr_state`, with the known MR as a child even when
merged or closed. Task subtrees include done work. Loose sections include clean
merged checkouts and open MRs even without a ticket or local checkout; only
inactive Jira-only entries are suppressed. `--cached` uses the last remote data
without network. Snapshot warnings include unavailable Herdr/hc inventories and
skipped notes with their paths.

Open MR rows, at every nesting level, add `ci` (head pipeline status or null),
`approvals` (approver count), `unresolved` (discussion count), `draft`,
`merge_status` (GitLab's detailed status or null), `conflict` and `ready`.
`ready` requires successful CI, no draft/conflict/unresolved discussions and
`merge_status == "mergeable"`. Missing review data gives null CI and zero counts,
so readiness stays false. Merged/closed MRs omit these fields; `body` is retained.

Each tree adds `diff: {adds, dels, files}` relative to its merge-base with
`origin/HEAD`, or `main_branch = "main"` in `[[project.repo]]` when origin/HEAD is
unavailable. It includes committed, staged and unstaged tracked changes, using
local Git only, even with `--cached`. Zero changes gives zero counts; detached
trees, missing defaults and Git errors give null.

Agents can run `berbr add [<task>] DEMO-1 --name followup --agent codex
--description "Implement the followup"` to open another branch and agent in the
task's Herdr space. The task defaults to the current worktree/space. An existing
ticket requires `--name` for a new worktree or `--in <tree>` to reuse one; an
existing path requires `--in`. Without Jira, supply `--name` or `--in`.
With `--in`, the key is optional: `berbr add [<task>] --in <tree>` starts a fresh
agent briefed from the task without changing its tickets. The panel's worktree
menu uses this for **New agent**; **Resume agent** continues the last session.
Without a TTY, `add` never prompts and automatically behaves like `--bg`, keeping
the caller's focus. It waits for worktree creation and the agent launch request,
then prints one line with the path, tab and terminal IDs. Errors exit nonzero
with one error line. Herdr is required; a failed launch leaves the worktree for
inspection/retry with `--in`.

On a worktree without a task, **New agent** / **New terminal** run
`berbr open <path> [--shell]`: the configured agent without a brief (or a shell)
in a space named after the branch, after making sure the project's main-checkout
space exists so the sidebar nests it.

New task checkouts reserve a unique name before launch. A slug collision adds
its task ID (then a counter if needed); a new task never reuses another task's
checkout. Saved checkout paths include their repository; legacy bare names are
accepted only when unique. Newly discovered sessions also save full checkout
paths; versioned string entries preserve names containing spaces. For `add --in`,
use a checkout owned by that task.

New checkouts use `<main checkout>/.herdr/worktrees/<name>`. Configure
`[workflow] worktree_dir = "dev/trees"`, or override `worktree_dir` on a
`[[project]]`. It must be a repository-relative directory without `..`.
Discovery and resume keep using existing `.claude/worktrees` alongside the
configured root; existing checkouts are never moved. Names and pending name
reservations in either root prevent a conflicting checkout in the other.
The first checkout appends `/.herdr/worktrees/` (or the configured root) once to
that repository's `.git/info/exclude`; `.gitignore` is unchanged.
