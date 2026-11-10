# Closing a Berbr task

`berbr -p PROJECT done TASK --dry-run --json` returns a plan, even when blocked
(exit 0 does **not** mean safe). Review the whole plan. It must identify the exact
task and expected worktrees/tickets, with no blockers, dirty or kept trees,
known zero uncommitted counts and known unpushed counts. Normally unpushed must
be zero; the merged MR exception below covers squash/rebase. Unexpected tickets or worktrees require
resolution before execution. A clean plan alone does not establish that code
landed; verify remote/provider results first, as described in the entrypoint.

Berbr removes worktrees, transitions eligible Jira tickets, archives its note,
then closes the task's Herdr space and associated panes. Jira tickets not eligible
for closure remain open; report `leave_tickets` rather than closing them manually.
Do not assume GitHub issues need a Jira-style transition. A Jira failure can occur
after worktrees were removed, and a UI failure after the note was closed.

## Guarded runner

Use the bundled `scripts/close_task.py` with Python 3. It does not merge code or
verify remote landing. It captures the reviewed cleanup scope and worktree HEADs,
copies itself outside removed paths, then rechecks that scope, clean Git state and
HEADs immediately before invoking **only** `berbr -p PROJECT done TASK --yes`.
Quiesce task writers first: the helper and Berbr are not a shared transaction with
other Git processes. A changed plan stops execution. It never forces cleanup.

From a stable directory outside every removed worktree, choose a new private
state directory outside them (a durable user state directory is preferable;
temporary storage survives worktree deletion but may be purged later):

```sh
python3 /path/to/done/scripts/close_task.py prepare \
  --project demo --task demo-7 --cwd /path/to/main-checkout \
  --state-dir /path/to/state/done-demo-7
```

Read `manifest.json` there and compare its plan with the intended task scope,
including `close_tickets` and `leave_tickets`. The directory must be new; it
contains a stable script copy, manifest, `launch.json` argv and `receipt.json`. Preparation itself
does not close anything. Use a trusted installed `berbr` executable; `--berbr`
can select an explicit executable when required. Preserve the same Berbr config
environment for preparation and execution. The generated launch argv explicitly
carries the saved allowlisted config/backend variables (including the core socket)
because hc programs inherit the core's environment rather than the calling
agent's. Inspect those assignments along with the task scope. It does not copy
credentials or the full environment. Never put this directory in a task
worktree, even if it belongs to a different repository.

After squash/rebase with a deleted source branch, Berbr may count commits against
the target's ancestry and report positive `unpushed` for already landed work.
Only when fresh provider evidence confirms **MERGED and the exact current source
HEAD**, pass `--merged-mr-head /absolute/task/worktree=FULL_SOURCE_SHA` to prepare
(repeat per affected worktree). Record the MR link, head, merge/squash result and
target verification in the task note. The helper requires `merged: true`, captures
the attested HEAD and checks it against Git again at execution. This option is an
agent assertion based on provider evidence, not an independent provider lookup;
never supply it for a closed/cancelled MR or merely to silence a positive count.
Unknown unpushed counts or Berbr blockers always stop cleanup.

Read `hc --skill` for the installed runner's current contract. Launch a visible
**plain program**, outside the task Herdr space, with no task/space labels and
`--outlive-parent`. This is cleanup, not a new coding agent:

```sh
python3 - /path/to/state/done-demo-7/launch.json <<'PY'
import json, subprocess, sys
with open(sys.argv[1]) as source:
    argv = json.load(source)
subprocess.run(argv, check=True)
PY
```

The generated argv uses `hc run --bg --no-parent --outlive-parent`, a stable cwd
and an explicit cleanup label so it does not inherit task placement. It launches
the stable Python/script copy with the prepared environment using `env`. Review
the actual argv before execution; use the harness's normal approval mechanism.
Verify the terminal is outside the task's UI space.
`--outlive-parent` only protects against parent exit; attaching the runner to the
space Berbr closes would still kill it. Do not launch through a helper to evade
sandbox permissions or an approval rejection. Request the necessary scoped
execution access through the harness when required. If `hc` is unavailable, use
an already authorized equivalent that survives caller/space exit, or leave cleanup
blocked. Never stop/restart the user's core or mass-kill sessions.

## Result and recovery

`receipt.json` records `prepared`, `closing`, `blocked`, `closed`, `partial` or
`error`; `done.log` preserves Berbr output. A per-directory lock prevents duplicate
execution. `closed` requires a successful command, `task show` confirming
`done: true`, and all planned worktree paths gone. A missing final receipt or
`closing` means unknown completion, not success.

On partial/error/interruption, inspect the log, task note (`task show` also finds
archived tasks), remaining worktrees, Jira and Herdr state before deciding how to
recover. Do not delete a lock or rerun cleanup blindly. If note/worktrees closed
but UI cleanup failed, report that separately. Keep the receipt/log accessible
after the agent exits; remove only your own helper terminal after collecting its
result, if the caller survives.
