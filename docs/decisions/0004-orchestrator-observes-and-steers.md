# ADR-0004: The orchestrator observes OpenCode's loop and steers it through hooks

- **Status:** Accepted (2026-10-26)
- **Date:** 2026-10-26 (proposed, revised after review, accepted the same day)
- **Deciders:** Kushagra Srivastava

## Context

Release 1 runs inside OpenCode, which owns the agent loop, sessions and the interface (ADR-0001). The harness
therefore cannot call the model itself or stop a turn. The workflow needs three things that the harness can provide from
outside the loop: a task state, loop detection, and a decision for the next step. The design calls for a bounded,
explicit workflow, which the harness cannot enforce by controlling the loop.

Three facts constrain the design. Each hook call is a separate `agent hook` process, and OpenCode may run tool calls in
parallel. OpenCode does not pass MCP tool output to `tool.execute.after`, so test results are visible only to the
`agent mcp` server process. `tool.execute.after` cannot deny a call.

The alternatives considered are listed below.

## Decision

1. **Observe and steer.** The harness observes every tool call and tool result through the hooks and the MCP server,
   keeps the task state, and steers only through a verdict on `tool-before` (allow or block) and the text of tool
   results. Phases are inferred from observed events, not declared by the model. Every block states the rule that
   fired and the next action that is still allowed. The decision logic is deterministic (rules only).
2. **The event log is the source of truth.** Each task has an append-only event log. Every record has a per-task `seq`,
   and records are ordered by `seq`, not by time. `task.json` and `state.json` are caches that one pure fold function
   rebuilds from the logs. Every append and every cache write holds an exclusive lock file (`std::fs::File::lock`), held
   for the shortest time possible.
3. **State failures degrade; policy stays fail-closed.** If the state cannot be read or written, the call is decided by
   the policy alone, a notice appears in the next tool result, and a record is written when the log is writable.
   `agent task repair` rebuilds the caches. A policy that cannot be evaluated still denies the call.
4. **Compatibility.** Unknown fields are tolerated. A newer major schema version is refused with a named error. An
   older one is migrated forward, keeping a `.bak` copy. Each JSONL line carries a version. A torn last line is skipped
   and counted. Unknown enum values decode without failing. Fingerprints carry a `fp_v1:` prefix.
5. **The MCP server records its own results.** `agent mcp` appends each result to its own log. The `tool-after` hook for
   the same call claims the result by tool name and argument hash and copies it into the task log.
6. **Task identity.** Task ids are assigned by the harness, not taken from session ids. Resume is by task id. A child
   session joins its parent's task when the host payload names the parent, and is a separate task otherwise. Session
   ids must match `[A-Za-z0-9_-]` with a length cap before they are used as keys or file names; anything else is
   rejected.
7. **Workflow switch.** `[workflow].enabled = false` may be set only in the machine config. A repository config may
   enable the workflow or tune its thresholds, but cannot switch it off. The trust rule that enforces this is part of
   the `[workflow]` configuration.

## Consequences

### Positive

- OpenCode keeps the loop, sessions and interface, so the harness does not duplicate them.
- The same events give the same state and decision, which makes loops testable.
- A model that forgets to report its phase cannot stall the state.
- Parallel hook processes cannot lose or reorder each other's records, and a damaged cache is rebuilt from the log
  (decision 2).
- A state bug or a locked file costs loop detection for some calls, not the whole session (decision 3).
- Old state files keep loading after an upgrade, and a downgrade is refused by name instead of misread (decision 4).
- Test results reach the state although the after-hook never sees MCP output (decision 5).
- Host-supplied ids cannot name a path outside the state directory (decision 6).
- A repository cannot switch off the loop refusals that the machine owner chose to keep (decision 7).

### Negative

- The harness cannot stop a model from producing text. It can refuse tool calls, and nothing more.
- Steering depends on the order in which OpenCode sends hook events. The black-box fixtures must cover that order.
- Each refusal costs a turn for the model. The refusal text has to be short and specific.
- Every hook takes a lock, which adds latency and a wait when calls run in parallel (decision 2).
- While degraded, a real loop can go undetected (decision 3).
- Migration code must be kept for each older major version (decision 4).
- Two identical MCP calls in flight at once can have their results swapped (decision 5).
- A child session whose parent is not named in the payload is tracked as its own task (decision 6).
- Switching the workflow off for one repository means editing the machine config (decision 7).

## Alternatives considered

- **Run the loop inside the harness.** Rejected: it duplicates OpenCode and contradicts ADR-0001.
- **Steer with instructions in the system prompt only.** Rejected: the result depends on the model following the
  instructions, which is what the harness is meant to avoid.
- **Fail closed on state errors.** Rejected: one unreadable file would block every call until a person intervenes,
  and loop detection is a heuristic, not a security control.
- **Mutable state files without a log.** Rejected: parallel hook processes would overwrite each other, and a corrupt
  file could not be rebuilt.

## Related

- [ADR-0001: Rust core with a thin host adapter](0001-rust-core-thin-host-adapter.md)
- [Security](../concepts/security.md): the fail-closed policy that decision 3 leaves unchanged
