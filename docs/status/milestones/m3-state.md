# M3: State, loop detection and the first decision engine

M3 gives the harness its own record of a task. It stores the task state, detects when the model is going round in a
loop, and records every decision with its reason. A session can then be resumed without rebuilding the transcript.
This page holds the plan. Steps will be added to it as they are built, in the same way as
[M2](m2-tools.md).

> **Status:** Done and committed as `736fea3` (2026-10-26); steps 0 to 11 complete. Gate green
> (`wgate`: fmt, clippy with warnings denied, 614 tests). Live runs with Qwen3.5-9B found the limits listed under
> [Result](#result-2026-10-26). The choices are in [Decisions](#decisions).

## Goal

OpenCode owns the agent loop, so the harness cannot stop a model from taking a turn. It can observe every tool call and
tool result, keep a record of them, and answer each tool call with a verdict. M3 uses those three capabilities to:

1. keep the task state (phase, iteration, last action, last test, failure history) in `.local-agent/`,
2. recognise a repeated failure or a repeated action as a loop, and refuse the next identical call with the reason,
3. decide the next step with a deterministic rules engine, and record the decision,
4. work in a separate Git worktree when a task is autonomous (R-26.1).

M3 does not use a model to make decisions. The LFM decision engine is M4, and the Jev backend is M7.

## Scope

| Requirement | Delivered by |
|---|---|
| R-16.1 state files under `.local-agent/` | the task event log, `task.json`, `state.json`, `decisions.jsonl`, `failures.jsonl` (see [State files](#state-files-and-the-owner-of-every-write)); plan and summaries in later milestones |
| R-16.3 reconstruction from Git and logs | `task.json` and `state.json` are caches rebuilt from the task event logs by `agent task repair` (steps 1 and 8) |
| R-16.4 `TaskState` fields | every field listed in the requirement, defined in step 2 |
| R-17.1 bounded workflow | phases START, DISCOVER, PLAN, IMPLEMENT, VERIFY, REVIEW, DONE, FAIL, DIAGNOSE, EXPLORE, ESCALATE as an explicit transition table |
| R-17.2, R-17.3 limits | iteration and retry limits in config (`[workflow]`), with the defaults from R-17.3 |
| R-9.2 to R-9.5 engine interface | `DecisionEngine` trait, `decide(state, schema) -> DecisionResult`, `RulesDecisionEngine` only |
| R-10.1, R-10.2, R-10.3 vocabulary | the enum values M3 can produce; no free-form control text |
| R-19.1 to R-19.3 failure fingerprints | normalised fields, stable SHA-256 hash with a version prefix, shared by `parse_test_failure` output |
| R-20.1 loop detection (partial) | four of the six cases: repeated command, repeated failure, same file rewritten, same test failing across attempts. Repeated patches and no metric progress are M4. |
| R-26.1 to R-26.4 worktree mode | `agent task start --worktree` creates `.worktrees/agent-task-NNN/` for autonomous tasks |
| R-49.1, R-49.2 git record | branch, base commit, `git status` before the first mutation, diff and files changed at the end |
| R-54.2, R-54.3 inspection | `agent explain` and `agent trace`, both bounded |
| R-57.3 resume | resume by task id: `agent task resume <task_id>`, `agent task status` and the `task_status` tool return the state without the transcript |
| AC-A6 repeated failures are detected | step 4 tests and the end-to-end run |
| AC-O3 deterministic fallback works | step 5: the rules engine is the only engine in M3 |
| AC-S2 worktree mode works | step 9 tests |

Requirements tagged M3 that M3 does not complete: R-20.1 (two of six cases, see above), R-20.2 (STALLED handling, M4),
R-29.1 (routing beyond MAIN, M4 and M5), R-41.1 (run record, seeded in M3 and completed in M10), R-28.2 (metrics beyond
per-event fields, M10), R-22.1 (research mode schema, M10). These stay open in the register and are marked as partial in
the step that touches them.

## Design rules

- **Observe, then steer.** Observers append events to a log. The task state is folded from that log. The harness steers
  only through verdicts on `tool-before` and through the text of tool results.
- **The event log is the source of truth.** Each task has an append-only event log. `task.json` and `state.json` are
  caches over the logs and can be rebuilt from them at any time (R-16.3).
- **Order by sequence number.** Every record carries a per-task `seq`. Records are ordered by `seq`, never by wall
  clock. Timestamps are kept for people, not for ordering.
- **Phases are inferred, not declared.** A phase changes because of an observed event: a file edit moves to IMPLEMENT,
  a verification step moves to VERIFY, a passing verification with no later edit moves to REVIEW. The model does not
  have to report its phase, so a model that forgets cannot stall the state.
- **Every block carries a reason and the next allowed action.** A refused call says why, and names a call that is
  still allowed. A refusal never fails silently (R-10.3).
- **Policy fails closed; state degrades.** Policy verdicts stay fail-closed, as in M2. A failure to read or write the
  state does not block calls: the harness falls back to M2 behaviour and says so (see
  [Failure handling](#failure-handling)).
- **Redaction stays on.** Every free-text value written to a state, decision or failure file passes the redactor and the
  `max_field_bytes` cap, as the event log does (see [Redaction](#redaction)).
- **Deterministic.** The same event sequence gives the same state, the same decision and the same fingerprint.
  Fingerprints do not depend on time, process ids or temporary paths.
- **No new crate.** SHA-256 uses `sha2`, already a workspace dependency. File locking uses `std::fs::File::lock` and
  `File::try_lock`, which are stable on the pinned Rust 1.95.
- **Off by default.** `[workflow].enabled` defaults to `false`. With it off, the hooks and the MCP server behave exactly
  as in M2.

## Observation points

Each hook call is a new `agent hook` process started by the shim (`opencode-plugin/local-agent.ts`), and OpenCode may
run tool calls in parallel. OpenCode does not pass MCP tool output to `tool.execute.after` (0 bytes observed on
2026-10-21, see [progress.md](../progress.md)), so test results and fingerprints must be recorded by the MCP server.

| Signal | Process that sees it | Data available | Record appended |
|---|---|---|---|
| Any tool call, before it runs | `agent hook tool-before` | tool, session id, call id, arguments, policy verdict | `call_started` (with verdict and command hash) |
| Built-in tool result | `agent hook tool-after` | tool, session id, call id, arguments, output, duration | `call_finished` |
| File edit | `agent hook tool-after` for `edit`, `multiedit`, `write`, `patch` and `apply_patch`; `agent hook event` for `file.edited` | path | `file_edited` |
| Shell command | `agent hook tool-before` (command text and class); `agent hook tool-after` (output) | no exit code in the payload | `call_started`, `call_finished`; a test runner's output also goes through the M2 failure parsers and gives `verification_result` with outcome `failed` or `unknown`, never `passed` |
| MCP result (`run_verification`, `run_tests`, `run_linter`, `run_typechecker`, `parse_test_failure`) | `agent mcp` (the long-lived server process) | tool, arguments, structured result, fingerprint | `mcp_result` in `.local-agent/mcp-results.jsonl` |
| MCP call completion | `agent hook tool-after` (no output) | tool, session id, call id, arguments | `call_finished`, then `verification_result` copied from the matching `mcp_result` |
| Session start and end | `agent hook event` (`session.created`, `session.idle`, `session.error`) | session id, parent session id if present | `session_bound`, `session_idle`, `session_error` |
| Task commands | `agent task ...` (user CLI) | prompt, flags, git state | `task_started`, `task_resumed`, `task_reset`, `repaired` |

The MCP server does not see the OpenCode session id. It records each result with the tool name and a hash of the
canonical arguments. OpenCode calls `tool.execute.after` for an MCP call only after the server has returned, so the
`tool-after` hook for that call finds the result already in `mcp-results.jsonl`. It claims the oldest unclaimed record
with the same tool and argument hash and appends a `verification_result` to the task log, with the `mcp-results.jsonl`
`seq` it claimed. Claims are cached in `state.json` and can be rebuilt from the task logs.

Every observer only appends. One pure function, `fold(state, record) -> state` in `harness-core`, turns records into
state. It runs in whichever process holds the task lock, right after that process's append.

## State files and the owner of every write

| File | Kind | Written by | Lock |
|---|---|---|---|
| `.local-agent/tasks/<task_id>/task-events.jsonl` (and rotated segments) | source of truth | `agent hook` processes, `agent task` commands | task lock |
| `.local-agent/tasks/<task_id>/decisions.jsonl`, `failures.jsonl` | copies of the decision and failure records, same `seq` | the process that appended the record, in the same locked section | task lock |
| `.local-agent/tasks/<task_id>/task.json` | cache: `TaskState`, last folded `seq`, recent-call ring | the process that appended, after the fold | task lock |
| `.local-agent/mcp-results.jsonl` | log of MCP results, own `seq` | `agent mcp` only | workspace lock |
| `.local-agent/state.json` | cache: session-to-task bindings, next task number, worktree list, MCP claims | `agent hook` on a new session or a claim; `agent task` commands | workspace lock |
| `.local-agent/state.lock`, `tasks/<task_id>/lock` | lock files | any of the above | n/a |
| `.local-agent/tool-events.jsonl`, `events.jsonl` | M2 telemetry, unchanged | `agent hook` | none (M2 behaviour); not read for state |
| `.worktrees/agent-task-NNN/` | worktree | `agent task start --worktree` | workspace lock for numbering |

Locking rules:

- Every append and every cache write holds an exclusive lock on a lock file, taken with `File::try_lock` in a loop for at
  most 2 s (proposed). A process that cannot get the lock in time goes to degraded mode.
- The lock is held for the shortest time possible: read the cache, append, fold, write the cache. Policy evaluation, git
  calls, parsing of tool output and tool execution happen before the lock is taken.
- Lock order is the workspace lock before a task lock, never the reverse.
- The operating system releases the lock when a process exits, so a killed hook does not leave the state locked.
- `seq` is assigned under the task lock: the last `seq` in `task.json`, checked against the tail of the log. If the log
  is ahead of the cache (a process died between append and cache write), the missing records are folded first.
- Task numbers and worktree numbers come from one counter in `state.json`, read and incremented under the workspace
  lock, so two `agent task start` commands cannot pick the same NNN.

All new files are under `.local-agent/`, which the policy already protects from agent writes. Step 9 adds `.worktrees`
to the protected root entries, so a session in the primary tree cannot write into a task worktree or its state.

## Writes on Windows

- Each cache file is written to `<name>.tmp` in the same directory, synced with `File::sync_all`, and renamed over the
  old file. The fixed temporary name is safe because only the lock holder writes it.
- A rename over a file that antivirus or the indexer holds open can fail with a sharing violation. The rename is retried
  5 times over about 300 ms (proposed), well inside the shim's `HOOK_TIMEOUT_MS` of 20 000 ms. After the last try the
  write fails with a named error and the process goes to degraded mode.
- At the start of every locked section, stale `*.tmp` files in that directory are removed.
- A process killed mid-write leaves the old cache intact and possibly a stale `.tmp` file, which the next writer removes.
  A process killed mid-append leaves a torn last line, which readers skip (see [Compatibility](#compatibility)).
- JSONL logs rotate at 8 MiB (proposed): the active file is renamed to a numbered segment under the task lock, with the
  same rename retry. Segments are kept, because the log is the source of truth. Only `agent task prune` deletes them.

## Compatibility

- Every JSON file has `"schema": {"major": 1, "minor": 0}`. Every JSONL line has `"v": 1`.
- Unknown fields are tolerated on read.
- A higher major version than the binary supports is refused with a named error (`StateError::NewerSchema`, naming the
  file and both versions). In a hook that means degraded mode, not a denial.
- A lower major version is migrated forward on the next write. The old file is kept as `<name>.v<major>.bak`. JSONL
  lines are never rewritten; the reader decodes each line by its `v`.
- A torn last JSONL line is skipped and counted. A malformed line elsewhere is also skipped and counted. Both counts are
  shown by `agent task status` and `agent task repair`.
- Unknown enum values (a record kind, a phase, a decision) decode to an `Unknown` variant. The fold ignores an unknown
  record kind. A cache with an unknown phase is treated as stale and rebuilt from the log.
- Fingerprints are written as `fp_v1:` followed by 16 hex characters, so a later change to normalisation gives
  `fp_v2:` values that never compare equal to old ones.

## Failure handling

| Failure | Behaviour |
|---|---|
| Policy cannot be evaluated (bad config, missing binary) | Denied, as in M2. Unchanged. |
| State cannot be read or written in `tool-before` (lock timeout, I/O error, newer schema, corrupt cache that cannot be rebuilt) | Degraded: the policy verdict alone decides, loop rules are off for that call, a notice is shown in the next tool result, and a `degraded` record is appended when the log is writable. |
| State cannot be written in `tool-after` | `tool.execute.after` cannot deny anything. The hook adds a one-line notice to the output of a built-in tool and leaves a `degraded` marker file if it can. The next `tool-before` sees the marker, or the same failure, and runs degraded. |
| State cannot be written in `agent mcp` | The result is returned with a one-line notice appended. The task state misses that result until `agent task repair`. |

A degraded call never blocks. The notice names the file and the error and suggests `agent task repair`. The next
successful locked write clears the marker and appends a `degraded` record covering the gap. `agent task repair`
rebuilds `task.json` and `state.json` from the logs (R-16.3) and reports skipped lines.

## Redaction

- Redacted before storage: `original_prompt`, `current_objective`, command text, `last_action`, `last_test`, the
  `git status` captured before the first mutation, failure messages, and every other free-text field. Identifiers
  (task, session and call ids), `seq`, enum values, counts and hashes are not redacted, as in the M2 event log.
- Command normalisation and fingerprint input run on the unredacted text, in memory, before redaction. Only the hash and
  the redacted text are stored. This choice is made so that two different commands, for example two that differ only in
  a secret value, cannot compare equal after redaction hides the difference.
- `agent explain` and `agent trace` replace control characters (C0, C1 and DEL, except newline in multi-line fields)
  with visible escapes before printing.

## Fingerprints and loop rules

A `FailureFingerprint` is built from the output of `parse_test_failure` (M2). Its fields follow R-19.2: the exception
or error kind, the command, the test identifier, the source location without line numbers when the file is the same, the
key message tokens after removing addresses and timestamps, the stack-trace signature, and the exit code. The fields are
joined in a fixed order and hashed with SHA-256; the first 16 hex characters are kept, prefixed with `fp_v1:`.

The loop detector reads a bounded ring of the last 16 calls (proposed) kept in `task.json`, not the full log. Each
entry holds the call id, tool, class, command hash, files edited and fingerprint.

Loop rules. The same-failure and implementation-retry limits are the R-17.3 defaults. R-17.2 requires the iteration
limit to be configurable; its default value is proposed. The other thresholds are proposals for step 4 and are tuned
from trajectories. All of them are configurable.

| Rule | Trigger | Decision | Source |
|---|---|---|---|
| Same failure | the third consecutive verification with the same fingerprint | ESCALATE (two retries are allowed) | R-17.3 |
| Implementation retries | IMPLEMENT entered more than 3 times for one objective | ESCALATE | R-17.3 |
| Repeated command | the same normalised command 3 times in the last 5 calls with no edit between; READ_ONLY commands are exempt | refuse the next identical call | proposed |
| Repeated edit | the same file edited 4 times in one iteration | ASK_USER | proposed |
| Iteration limit | `iteration` reaches `max_iterations` (default 10) | TASK_BLOCKED | R-17.2 (limit), proposed (value) |
| Refusal cap | 3 consecutive refusals by loop rules in one task | TASK_BLOCKED; loop refusals stop | proposed |

Re-running a test with no edit, for example a flaky test, is a legitimate retry. The repeated-command rule allows the
second and third identical runs, and the same-failure rule allows two retries, so a flaky test can be re-run twice
before any rule fires.

What each decision does:

| Decision | What the model sees | What the user sees | What ends it |
|---|---|---|---|
| Refuse repeated command | the call is refused: the rule, the count, and "edit a file or run a different command" | the refused call in the OpenCode session; the decision in `agent explain` | any edit, or the earlier copies leaving the 5-call window, or `agent task reset` |
| ASK_USER (repeated edit) | the next edit to that file is refused: "run verification, or ask the user" | the refused call; `agent task status` shows ASK_USER | a verification call (it starts a new iteration), or `agent task reset` |
| ESCALATE (same failure) | the next verification with no edit since the failure is refused, with the fingerprint, test id and file; later tool results carry a one-line notice | the refused call and the notice; `agent task status` shows ESCALATE. There is no deep model in M3 (M5). | an edit to a file named in the failure location or the test file (any edit if the failure names no file) lifts the refusal; a verification with another fingerprint or a pass ends the notice; or `agent task reset` |
| ESCALATE (implementation retries) | WRITE calls are refused: "implementation retries exhausted; report to the user"; READ_ONLY calls and `task_status` stay allowed | the refused call; `agent task status` shows ESCALATE | `agent task reset` |
| TASK_BLOCKED (iteration limit) | WRITE and EXECUTE calls are refused: "iteration limit reached; report to the user"; READ_ONLY calls stay allowed | the refused call; `agent task status` shows TASK_BLOCKED | `agent task reset`, or a higher `max_iterations` and `agent task resume <task_id>` |
| TASK_BLOCKED (refusal cap) | no further loop refusals; every later tool result carries a one-line notice naming the reason | the notice; `agent task status` shows TASK_BLOCKED | `agent task reset` |

The refusal cap exists because the harness cannot end a turn. Refusing without end only spends the model's turns, so
after the cap the harness records the block, stops refusing on loop grounds, and leaves the decision to the user. The
cap applies to every row above. Policy verdicts are not affected by it. The model cannot run `agent task reset` itself: `agent` is an unrecognised
program to the command classifier, so the call is EXECUTE_RISKY and blocked (D3).

## Task identity

- A task id is assigned by the harness (`task-NNN`, from the counter in `state.json`). It is not a session id.
- A session is bound to a task on the first event seen for it: to the task that `agent task start` or
  `agent task resume <task_id>` left waiting, otherwise to a new task.
- A child session (a subagent) gets its own session id. If the host payload gives a parent session id, the child is
  bound to the parent's task. Otherwise it is recorded as a separate task. A binding is not changed after it is made.
- A fresh OpenCode run gets a new session id, so R-57.3 resume is by task id: `agent task resume <task_id>` makes the
  next new session continue that task.
- Session ids come from the host payload and appear in `state.json` keys and file names. They must match
  `[A-Za-z0-9_-]{1,128}` (the length cap is proposed). Anything else is rejected: the event is not recorded as task
  state, and the call runs degraded with a notice. The policy verdict is unaffected.

## Steps

Each step ends with `cargo fmt --check`, `cargo clippy -D warnings` and `cargo test --workspace --locked` on Windows,
and with the CI run on `main`. Each step updates the register, this page, and the documentation named in its row.

| Step | Deliverable | Test | Documentation |
|---|---|---|---|
| 0 | This plan and [ADR-0004](../../decisions/0004-orchestrator-observes-and-steers.md) (proposed): the observe-and-steer model | review only | this page, the ADR |
| 1 | State store in `harness-runtime`: lock files, locked JSONL append with per-task `seq`, atomic cache writes with rename retry, stale `.tmp` cleanup, rotation, schema version and migration, redaction | write and read back; a killed writer leaves the old file; a torn last line is skipped and counted; a newer major version gives the named error; two processes appending in parallel give gap-free `seq`; on Windows, a target held open without `FILE_SHARE_DELETE` makes the rename retry, then succeed or fail with the named error | [architecture.md](../../concepts/architecture.md) "Where state lives" |
| 2 | `TaskState`, `Phase`, the transition table and `fold` in `harness-core` (pure) | every pair of phases checked against the table; illegal moves rejected; unknown record kinds ignored; the same records give the same state | module page in [reference/modules](../../reference/modules/README.md) |
| 3 | `FailureFingerprint` in `harness-core`, with the `fp_v1:` prefix | fixture outputs from pytest, cargo, gtest and tsc: same failure with moved lines gives one hash; different test ids give different hashes; commands that differ only in a secret give different hashes | module page |
| 4 | Loop detector: a pure function over the recent-call ring and a plain `LoopThresholds` struct with explicit defaults | one test per rule, the READ_ONLY exemption, a flaky re-run that must not trigger, the refusal cap, and a sequence that must not trigger | module page |
| 5 | `DecisionEngine` trait and `RulesDecisionEngine`; `[workflow]` config (`enabled`, thresholds, rotation size, lock wait) with validation rules V26 and later | same state gives the same decision; invalid limits rejected: V26 thresholds below their lower bound (a repeat count below 2, a retry or iteration limit or refusal cap below 1), V27 a window smaller than its count or larger than the ring, V28 a rotation size below 1 MiB (proposed bounds) | [configuration.md](../../guide/configuration.md), [core-config.md](../../reference/modules/core-config.md) |
| 6 | Hook wiring behind `[workflow].enabled`: observers append, `fold` runs under the lock, `tool-before` applies the loop rules, degraded mode and notices; `agent mcp` records its results; `tool-after` claims them. Ships disabled until step 10 passes. | black-box fixtures extended in `hook_fixtures.rs`: the block names the next allowed action; a held lock gives degraded mode, not a denial; a bad session id is rejected; with `enabled = false` every M2 fixture gives the same answer | [cli-host-opencode.md](../../reference/modules/cli-host-opencode.md), [runtime-mcp.md](../../reference/modules/runtime-mcp.md), [security.md](../../concepts/security.md) |
| 7 | `task_status` MCP tool (READ_ONLY) returning the state summary; disabled by default until step 10 | structured output is redacted and bounded | [runtime-mcp.md](../../reference/modules/runtime-mcp.md) |
| 8 | `agent task start`, `status`, `resume <task_id>`, `reset`, `repair`, `prune`; `agent explain`, `agent trace` with a record limit (default 50, at most 1000, proposed) | CLI tests on a temporary repository: repair rebuilds a deleted cache to the same state; prune removes only finished tasks; explain and trace escape control characters | [cli-commands.md](../../reference/modules/cli-commands.md) |
| 9 | Worktree mode: `agent task start --worktree` creates `.worktrees/agent-task-NNN/`, records the base commit, and generates `opencode.json`, the shim copy and (if untracked in the primary tree) `local-agent.toml` inside the worktree with the `agent init` code; `.worktrees` added to the protected root entries | the primary working tree and its `git status` are unchanged after a task; `.worktrees/` is in `.gitignore` (already written by `agent init`); two parallel starts get different NNN | [security.md](../../concepts/security.md), [configuration.md](../../guide/configuration.md) |
| 10 | End-to-end run with `enabled = true`: Qwen3.5-9B through OpenCode on the toy repository, with a failing test the model cannot fix in one attempt | the loop rule fires; the record is in the task event log and `decisions.jsonl`; the session resumes by task id from `task_status`; then `enabled` and `task_status` default decisions are revisited | [progress.md](../progress.md) |
| 11 | Result section on this page, register rows (R-20.1 marked partial), progress entry | the register and the progress log agree | this page, [requirements-traceability.md](../../reference/requirements-traceability.md) |

`agent task prune` removes the task directories and worktrees of finished tasks. The policy blocks `git worktree remove`
(an unrecognised git subcommand, so EXECUTE_RISKY), so removing a worktree must be an explicit user command.

## Decisions

Accepted on 2026-10-26 as proposed, except decision 7, which is changed. Decisions that shape the design are also
recorded in [ADR-0004](../../decisions/0004-orchestrator-observes-and-steers.md).

1. **Task boundary.** A new session starts a new task, unless `agent task start` or `agent task resume` left a task
   waiting. `agent task start` remains available for any task.
2. **Worktree default.** `agent task start --worktree` creates a worktree for autonomous tasks only. Interactive work
   stays in the primary tree (R-26.4). OpenCode is started inside the worktree path the command prints. The worktree
   task's state lives in the worktree's own `.local-agent/`, and the primary tree's `state.json` lists it.
3. **Thresholds.** The R-17.3 defaults, and the proposed values in the loop rule table. Tuned in M10.
4. **Refusal cap.** After 3 consecutive loop refusals, the harness stops refusing on loop grounds and shows a notice.
   `agent task reset` clears the counter.
5. **ESCALATE in M3.** The actuators in the decision table apply now. The deep model does not exist until M5.
6. **`task_status` default.** Disabled until step 10, then revisited with the step 10 result, because each tool schema
   costs prompt tokens on a 16K-context model.
7. **Who may set `[workflow]`. Changed from the proposal.** Any config file may set the thresholds. Only the machine
   config may set `enabled = false`. A repository config may enable the workflow, which only adds refusals, but it
   cannot switch the workflow off. The trust rule is added in step 5 (`check_repo_layer` in `config/trust.rs`).
8. **Numbers.** Lock wait 2 s; rename retry 5 tries over about 300 ms; ring of 16; rotation at 8 MiB;
   `explain` and `trace` limit 50 (at most 1000); session id cap 128; `max_iterations` 10. Accepted for M3 and tuned
   at step 10.

## Result (2026-10-26)

| Step | Result | Evidence |
|---|---|---|
| 0 Plan and ADR | Done | [ADR-0004](../../decisions/0004-orchestrator-observes-and-steers.md) accepted; commit `e6eef64` |
| 1 State store | Done | `crates/harness-runtime/src/state/`: atomic writes with a Windows rename retry, lock with a timeout, versioned envelopes, torn-line handling; 16 integration tests (`tests/state_store.rs`) including a Windows test that holds the target open |
| 2 Task model | Done | `harness-core/src/task.rs`: phases, the R-17.1 transition table, shortest-path advance; every pair of phases is checked |
| 3 Fingerprints | Done | `harness-core/src/fingerprint.rs`: fields of R-19.2 except the stack-trace signature (not in the parsed output); SHA-256, `fp_v1:` prefix |
| 4 Loop detector | Done | `harness-core/src/loops.rs`: one test per rule and the cases that must not trigger |
| 5 Decisions and config | Done | `decision.rs` (rules engine, DONE before any loop rule, refusal rule); `[workflow]`; validation V26 to V29; trust rule: a repository cannot set `enabled = false` |
| 6 Hook wiring | Done | `journal.rs` (records and fold), task store and MCP result recording in `harness-runtime`, hook glue in `harness-cli/src/workflow.rs`; 8 black-box tests in `crates/harness-cli/tests/orchestrator.rs` |
| 7 `task_status` | Done, off by default | `mcp/tools/task.rs`; offered only when `[workflow].task_status = true` |
| 8 CLI | Done | `agent task start|status|resume|reset|repair|prune`, `agent explain`, `agent trace` |
| 9 Worktrees | Done, prune of worktrees not done | `agent task start --worktree` creates `.worktrees/agent-task-NNN`, copies the generated config, binds the next session; the primary tree is unchanged (black-box test); `.worktrees` is a protected root entry; `git worktree add` is EXECUTE_RISKY, as the code already decided |
| 10 Live runs | Done, with findings | below |
| 11 Documentation | Done | this page, the progress log, the requirements register, configuration, architecture, security, README |

### Live runs (Qwen3.5-9B through OpenCode 1.19.3, toy repository)

- **Run 1** (`task-0001`): the model made a failing test pass by removing a contradictory assertion from the test
  file. Nothing stopped the edit; the trace shows the failed and then passed verifications recorded from MCP results.
- **Run 2** (`task-0004`): four identical `run_tests` calls. OpenCode's own `doom_loop` permission rejected the third
  identical call before the harness's tool-before hook ran, and the run ended. The harness recorded no refusal for
  this path. The journal recorded no outcome for the calls OpenCode cancelled, so none was guessed.
- **Run 3** (`task-0007`): an edit and re-test cycle on a guard test that cannot pass (two contradictory assertions in a
  protected `.opencode/` file). After three verifications with the same fingerprint, the next `run_tests`, `edit` and
  `run_command` calls were refused with the rule name and the allowed next step. The model then stopped and asked for
  help. The first version also refused `read`, which made the advice impossible to follow; that was fixed.
- **Run 4** (`task-0008`, after the fix): the model read the files and explained that the guard test cannot be fixed
  inside its constraints, and stopped. No refusal was needed.

### Deviations from the plan

- `max_iterations` is 8: the starting value of the brief's §17 in `[agent]`, which already exists. The plan said 10
  (decision 8); the brief's value is kept.
- Loop refusals never apply to read-only calls. The plan exempted them from the repeated-command rule only; the live run
  showed that the same exemption is needed for every loop rule, or the refusal cannot be followed.
- The refusal cap is reset only by an edit or a passing verification, not by any allowed call. Otherwise the cap
  restarts after every allowed call.
- The degraded notice is written to `tool-events.jsonl` and is shown by `agent trace` and `agent explain`. The after-hook
  cannot change an allowed call's result, so the notice cannot reach the model in the same tool result.
- No `git diff` at the end of a task (R-49.1 is partial: base commit and dirty-file count only). No pruning of
  worktrees; `agent task prune` removes task directories only.
- `plan.json` and the summaries directory of R-16.1 are not written yet.

## Limits

- The harness cannot stop a model from producing text. It can refuse tool calls, and a refused call is recorded.
- Loop detection is heuristic. It can refuse a legitimate retry, and the refusal states the rule that fired.
- Fingerprints depend on the output parsers of M2. A failure format that no parser recognises is hashed on its
  normalised text, which is coarser.
- A test run through the shell has no exit code in the after-hook payload, so it can record a failure but never a pass.
- MCP results are matched to calls by tool and argument hash. Two identical calls in flight at once can have their
  results swapped; this matters only when the results differ, as with a flaky test.
- A child session seen before its `session.created` event, or without a parent id in the payload, is recorded as a
  separate task.
- A stored command hash covers the unredacted command. A low-entropy secret in a command could in principle be guessed
  from the hash and the redacted text.
- Two sessions on one working tree are recorded as separate tasks, but their edits land in the same tree. M3 does not
  isolate them; worktree mode does.
- The M2 telemetry logs (`tool-events.jsonl`, `events.jsonl`) are not rotated or locked in M3.
- No model is used for any decision in M3. Decision quality is limited to the rules.

- OpenCode's `doom_loop` check ends a run of identical consecutive calls before the harness can refuse the third
  (run 2). Identical-call loops are therefore stopped by the host; the harness rules cover loops in which the calls differ,
  such as the edit and re-test cycle (run 3).
- The model can weaken a test to make it pass (run 1). Nothing in M3 prevents it. A protected test location
  (`.opencode/`) prevents the edit; a plain test file does not.
- The harness cannot stop the model from producing text, and the refusal is visible only through tool results and the
  trace. A refused model that does not stop keeps trying until the host or the user stops it.
- The task lock is a polling lock with no fairness. Under sustained contention a waiter can time out after 2 s, and the
  hook then degrades to the M2 policy. Hook calls come from separate processes spaced in time, so this is rare in use.

## Related

- [M2: Deterministic tools](m2-tools.md): the tools whose output M3 fingerprints
- [ADR-0004](../../decisions/0004-orchestrator-observes-and-steers.md): how the orchestrator relates to OpenCode's loop (proposed)
- [Requirements register](../../reference/requirements-traceability.md): the M3 rows
- [Architecture](../../concepts/architecture.md): where state lives today
- [Security](../../concepts/security.md): protected control files and fail-closed policy
- [Progress log](../progress.md): the evidence for each step
