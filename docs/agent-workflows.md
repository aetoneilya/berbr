# Agent workflow investigation

This change is based on local CLI/mod logs and the corresponding command/result
pairs in Claude and Codex sessions. Published examples use synthetic identifiers;
raw transcripts, task descriptions, credentials and unrelated project data are
not distributed. Exact private transcript pointers remain in the task's Agent
notes. This document distinguishes observed events from proposed capabilities.

## Evidence and changes

The initial CLI log snapshot contained 2,143 lines from November 6–9, 2026, with
16 `failed` records. The mod snapshot contained 142 lines with 11 nonzero runs:
five duplicate CLI events and six snapshot failures. These are observations from
one installation, not a population-wide failure rate or independent agent count.

| Case | Observation and source | Impact | Remedy / status |
|---|---|---|---|
| E1: task context | Six CLI refusals: log lines 1671, 1675, 1677, 1679, 1730, 1740; November 7, 11:30–14:10 local time. Session command/results corroborate lookups from the main checkout and a delegated checkout. | Findings were not appended; some retries guessed task names. | Skill uses project + stable task ID outside known context; snapshot discovery uses task rows' `task` field. Existing explicit-binding routing was already corrected before this change; no new routing fix is claimed. |
| E2: note syntax/help | The 14:10 attempt used `note demo-0 TEXT` after help was cut to eight lines, hiding Options. This overlaps E1 rather than adding another event. | The task ID became note text, and task resolution still failed. | Skill and generated brief use `note --task ID TEXT`; recovery guidance calls for complete subcommand help. Generated commands carry the selected project even for legacy notes without a project field. |
| E3: batch arguments | Three CLI refusals at lines 2039, 2041, 2043; November 9, 19:39:50. A session used zsh `set -- $spec`, losing the ticket argument, then succeeded three times with explicit function arguments. | Three workers were not launched; piping to `tail` obscured status. | Recovery shows quoted explicit arguments. An isolated argv stub reproduced both the failure and corrected vectors without launching agents. This is a shell error, not an `add` parser defect. |
| E4: sandbox writes | Two independent Codex sessions on November 9, 21:40/21:44 hit `Operation not permitted` at startup `task update`; a scoped-access retry is corroborated as successful. The investigating session reproduced it too. | An unnecessary update required permissions outside the checkout. | CLI skips the note write and shared write lock for unchanged fields. Real updates retain locking/reload/retry. Brief skips startup updates when fields exist and describes scoped-access recovery. A live sandbox replay succeeded with the task note's SHA256 unchanged. |
| E5: naming | Three refusals at CLI lines 1759, 1761, 1779 and mod lines 122–129; November 7, 15:09/15:19 local time. Error text had no reason after the colon. | The user/agent could not distinguish configuration, account or process failures. | Naming errors include the CLI, process status and bounded structured/plain stdout or stderr. Tests cover empty output and a successful process carrying an error envelope. The original external CLI failure's cause remains unknown. |
| E6: snapshots | Six mod `app-snapshot` runs ended with `-1`, with empty diagnostic lines: mod lines 1, 3, 17, 19, 131, 133. | There was no useful explanation to investigate. | The bridge records a nonempty missing-status/output message and awaits failure-log writes. This improves evidence; it does not establish a timeout, signal or underlying runtime fix. |
| E7: close guards | Four CLI refusals at lines 1649, 1723, 1880, 1887 protected unfinished checkouts. | Cleanup stopped with blockers. | Expected protection. The skill does not suggest forcing a refusal; destructive actions require their own authorization. |

CLI sources are `~/.berbr/logs/berbr.log`; mod sources are
`~/.config/herdr-app/logs/berbr-mod.log`. Line numbers refer to the initial
snapshots, before this investigation appended its own commands. CLI times use
the logged local offset; mod timestamps use UTC. Mod duplicates are not counted
as independent failures.

The old local skill also disagreed with the current CLI in three inspectable
contracts: `task show` does not return tickets/worktrees; `add --in` shares a
branch rather than creating a stacked MR; direct `hc` Codex launches require
`--no-daemon`. These are verified instruction defects, not additional observed
failures. The shipped skill corrects them and avoids a fixed permission policy.

## Delivery

`skills/berbr/` is the portable source: one skill, one recovery reference and
optional agent UI metadata. README describes installation without silently
replacing an existing skill. New skills for individual errors are unnecessary:
these cases are conditional recovery steps within the same Berbr workflow.

The generated brief contains the minimal context/recording commands even when
the full skill is not installed. It preserves the user's supplied requirements
and existing authorization. The skill is not automatically installed by the
CLI/mod installer, and old sessions do not acquire new permissions automatically.

## Follow-up CLI capabilities

The initial skill exercise confirmed the ticketless `add` limitation. The
subsequently authorized implementation adds all three proposed capabilities:

- `context --json`: resolve project/task/worktree/branch/brief locally, returning
  candidates on ambiguity. E1/E2 show the cost of reconstructing it.
- `task list --json`: expose stable IDs and titles without the full board.
- Ticketless `add --name`: create a new checkout within an existing task and
  save its explicit binding. This closes a verified coverage gap; that gap was
  not the cause of the zsh argument error in E3.

See [the agent CLI contract](agent-cli.md) for fields, exit status, local-only
resolution limits and partial failure behavior. The skill retains a fallback
for older installed binaries.

Agent launching/waiting/inspection already belongs to `hc`; duplicating it is
not required by these cases. Further naming/snapshot behavior changes need
evidence from the improved diagnostics, rather than guesses about the old runs.

## Validation and limits

Regression tests first reproduced the no-op lock failure, blank naming error and
empty snapshot diagnostic. The CLI fixes passed 232 Rust and 223 JS tests, plus
format/diff checks and an independent code review. Instruction checks additionally
exercise commands from a generated brief and use a fresh agent with an isolated
synthetic task/config and command capture; results are recorded in Agent notes.

Coverage does not include production access, remote machines, every shell,
account/provider outage recovery or human usability studies. Read operations are
not uniformly logged without debug, and clap argument-parse failures happen
before CLI logging starts. Command strings that only appeared in tool output
were not treated as executed commands; negative mod statuses were not assigned
an unobserved cause. No claim is made that every agent failure is fixed.
