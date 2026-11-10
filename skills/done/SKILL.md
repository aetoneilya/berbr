---
name: done
description: >-
  Finalize a completed task on request: land changes through its PR/MR or
  direct-to-main workflow, explain CI or approval blockers, then close the exact
  Berbr task and its worktrees gracefully. Use for /done, $done, or an explicit
  request to merge and close a task; creating this skill or asking for status
  does not execute it.
---

# Finish the task

Treat an explicit completion request as authorization for the ordinary commits,
pushes, merges and Berbr cleanup necessary for this task, subject to the user's
scope, repository rules and execution permissions. Reuse authorization already
given; do not add a confirmation after every step. If only discovered while
answering a question, prepare the result and obtain completion authorization
before mutations. Never bypass a harness denial through another process.

## Establish what is finished

Resolve the exact Berbr project and stable task ID, read its note and brief, and
inventory **all** task worktrees and repositories. Prefer `berbr context --json`
(`status` must be `resolved`), `task show` and `app-snapshot --cached`. With older
CLIs, use the supplied brief and snapshot task IDs; do not infer an ID from a
title or branch. Select the project explicitly for later commands.

Check the requested outcome and required checks against the actual diff. Reuse
still-valid test/review evidence. If implementation remains unfinished, continue
the authorized work; report failures or decisions that need the user. Preserve
unrelated changes. Commit only task changes, without attributing them to AI.
Do not discard another agent's work to make cleanup possible.

Determine each repository's actual target branch and workflow from current
instructions, Git remotes and provider state. “Master” can mean `main`; verify
the default/required target. An existing PR/MR takes the provider route. A known
pet project with direct-merge policy takes the direct route. Absence of an MR,
GitHub hosting or a folder name alone does not establish direct-merge policy.
If the route cannot be established, ask one focused question while completing
independent checks. Do not push an MR branch directly to main to avoid gates.

## Land every task change

Use the available provider connector, or its installed CLI when needed. Inspect
the exact repository, source branch, target and **latest head**, rather than
matching titles. Push task commits normally; create a PR/MR if the repository
requires one and none exists. Follow repository merge strategy and templates.

For an open PR/MR, read fresh CI, required checks, approvals, draft/conflict and
discussion state. Merge through the normal provider operation only when allowed.
Do not use admin/force options, disable checks, self-approve, or send teammates
messages/comments to obtain approval. Never infer approval from stale CI on a
previous head. If the provider cannot supply reliable status, leave it open.

When blocked, leave Berbr and the worktrees open and report the precise state
with a PR/MR link: CI running/pending (name the jobs), CI failed (name failures),
missing approvals (counts if available), conflicts, draft, unresolved discussions
or inaccessible status. Report simultaneous blockers together. Do not call a
failed pipeline “running”, mark the task done, or wait indefinitely. Continue
waiting only when the user requested monitoring or a bounded active wait is
useful; never silently enable auto-merge that would leave cleanup unattended.

For direct merge, integrate into the verified target using a clean checkout
outside the task worktrees (or a temporary integration checkout). Fetch first,
preserve concurrent target commits and local edits, merge normally, run the
relevant checks, then push without force. On a rejected push, fetch and reconcile
normally; unresolved conflicts or denied access leave the task open. Verify the
actual remote target contains the result; a local merge alone is insufficient.

Before cleanup, verify **every** task branch has landed. Provider `MERGED` and
the matching head/merge result are authoritative for squash/rebase workflows:
literal ancestry of the old branch SHA may be absent. A closed/cancelled MR is
not merged. If it contains unlanded work, stop; an explicit user decision is
needed to abandon it. Do not treat Berbr's `merged` boolean as proof of landing.

## Close gracefully through Berbr

Read [references/closing.md](references/closing.md) before cleanup. It describes
the guarded helper, fresh dry-run inspection, durable receipt and runner outside
the task space. Do not manually delete worktrees, move task notes, kill the
calling agent, or transition Jira separately: Berbr owns that sequence.

Record the landing/check evidence with `berbr -p PROJECT note --task TASK TEXT`
before its note is archived. Quiesce any task agents you started and inspect
their results. If other task agents are still writing, coordinate or stop cleanup;
never sweep unrelated sessions. Validate the full exact-task dry-run, including
dirty worktrees and ticket transitions. Never use `--force`, `--merged` or a
tree sweep for this workflow.

Give the user a short landing summary and the durable receipt/log location
**before** starting cleanup, since the task space may close the caller. Describe
cleanup as scheduled until observed. If the caller survives, inspect the receipt;
only `closed` proves complete cleanup. A partial failure needs state inspection,
not a blind retry. If safe detached execution or its permission is unavailable,
report “changes merged; Berbr cleanup blocked” with the concrete reason.
