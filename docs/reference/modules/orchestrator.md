# Module: orchestrator (M3)

The orchestrator keeps a record of each task, detects loops in the model's calls, and decides whether a call may
run. It spans three crates: the pure rules in `harness-core`, the storage in `harness-runtime`, and the hook and
command glue in `harness-cli`.

> **Status:** Built (M3 to M5). The rules decide every call. The micro model decides stalled loops (M4). The deep model is served on demand (M5). The context policy and the task block for the system prompt (M6) are in [context.md](context.md). Jev (M7) is not built.

## Responsibilities

| File | Public items | What they do |
|---|---|---|
| `harness-core/src/task.rs` | `Phase`, `TRANSITIONS`, `transition`, `path`, `TaskStatus`, `TaskState` | Phases of R-17.1, the legal moves, the shortest legal path, and the task fields of R-16.4 |
| `harness-core/src/journal.rs` | `Record`, `Entry`, `Outcome`, `Journal` | The facts about a task; `Journal::apply` folds them into the state and the ring of recent calls |
| `harness-core/src/fingerprint.rs` | `fingerprint`, `fingerprint_fields`, `command_hash`, `args_hash` | Failure fingerprints (`fp_v1:`), command and argument hashes |
| `harness-core/src/loops.rs` | `CallRecord`, `Thresholds`, `LoopSignal`, `detect` | The four loop rules and the iteration limit, as a pure function |
| `harness-core/src/decision.rs` | `DecisionEngine`, `RulesDecisionEngine`, `Decision`, `ReasonCode`, `refuses` | Turns signals into a decision; DONE before loop rules; refusal and cap |
| `harness-core/src/config/` | `WorkflowSection`, V26 to V29, the `enabled = false` trust rule | The `[workflow]` settings and their checks |
| `harness-runtime/src/state/` | `StateDir`, `FileLock`, `atomic_write`, `Task`, `SessionIndex`, `McpResult` | Versioned files, atomic writes, the lock, the task store, the session index |
| `harness-runtime/src/mcp/record.rs` | `record`, `read_result` | Records verification results from the MCP server before redaction |
| `harness-runtime/src/mcp/tools/task.rs` | `TaskStatusTool` | The `task_status` tool, off by default |
| `harness-cli/src/workflow.rs` | `before`, `after`, `refusal_message` | Maps hook payloads to records; applies the rules before an allowed call |
| `harness-cli/src/commands/task.rs` | `TaskAction`, `explain`, `trace` | The `agent task`, `agent explain` and `agent trace` commands |
| `harness-core/src/lfm.rs` | `CHOICES`, `parse_choice`, `is_stalled` | The contract of the micro model's answers: ESCALATE, ASK_USER or REPLAN; a strict check (M4) |
| `harness-runtime/src/decision.rs` | `LfmEngine` | The micro model for stalled loops, with fallback to the rules; the request holds no prompt text (M4) |
| `harness-cli/src/commands/eval.rs` | `run` | `agent eval-decisions`: the A/B run of the rules and a model on recorded cases (M4, M5) |
| `harness-cli/src/commands/models.rs` | `check_one_gpu_model` | `agent models serve` refuses a second GPU server while one runs (M5) |

## Behaviour

- Before an allowed call, `before` binds the session to a task, takes a snapshot of the journal, runs the rules, and
  either refuses the call (the refusal is journaled and the message names the rule and the allowed next action) or
  appends a `CallStarted` record.
- After a call, `after` records a verification result: for MCP verification tools, the result the server recorded for
  the same tool and argument hash, claimed once; for a shell test run, the failures in its output. An unknown outcome is
  never a pass.
- Every append holds the lock of the file it writes. The journal is the source of truth; `task.json` is a cache that is
  rebuilt when it is missing or damaged.
- A failure in the workflow keeps the policy's answer and writes a `workflow_degraded` entry to `tool-events.jsonl`.

## Tests

- `harness-core`: the transition table (every pair), the path search, the rules and their priority, the fold, the
  fingerprints, the validation rules V26 to V29.
- `harness-runtime`: `tests/state_store.rs` (16 integration tests) and unit tests of the state and the MCP recording.
- `harness-cli`: unit tests of the glue, and `tests/orchestrator.rs` (8 black-box tests on the real binary).

## Limits

- Loop rules are heuristics. Read-only calls are never refused by them.
- The rules see the calls the harness sees: built-in tools and MCP verification tools. A shell test run is parsed from
  its output; its runner's success is never inferred.
- `task prune` removes task directories only; worktrees are not removed.

## Related

- [M3 plan and result](../../status/milestones/m3-state.md)
- [ADR-0004](../../decisions/0004-orchestrator-observes-and-steers.md): how the orchestrator relates to OpenCode's loop
- [Security](../../concepts/security.md): residual risks 6 to 9

## Jev decision backend (M7)

`harness-core/src/jev.rs` checks the answer of the Jev service: one of the three stalled-loop choices, with a confidence
from 0 to 1 that is kept. `harness-runtime/src/jev.rs` holds `JevEngine`, which asks the service only for stalled loops,
and falls back to the rules on any failure. `harness-cli/src/workflow.rs` selects it in `decide_for`. The endpoint policy
is rules V30 to V35. Result and limits: [M7](../../status/milestones/m7-jev.md); design: [ADR-0007](../../decisions/0007-jev-is-an-optional-decision-backend.md).
