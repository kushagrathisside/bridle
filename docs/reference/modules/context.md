# Module: context (M6)

The context policy decides what the model sees of each completed tool result, and what the system prompt says about the
task. It spans three places: the pure rules in `harness-core`, the request transform and the task block in
`harness-cli`, and the hook forwarding in the shim.

> **Status:** Built (M6). Verified in a live run on 2026-10-26; see [the M6 result](../../status/milestones/m6-context.md).

## Responsibilities

| File | Public items | What they do |
|---|---|---|
| `harness-core/src/context.rs` | `ResultView`, `Action`, `Budget`, `plan`, `compact_text`, `compact_verification`, `stub` | The rules of the M6 page: KEEP, COMPACT or DROP per result, from age, size, kind and the budget. Pure. |
| `harness-core/src/telemetry.rs` | `EventKind::Context` | The kind of the per-request record, written to `events.jsonl`. |
| `harness-cli/src/host/opencode_v1.rs` | `messages_transform`, `scan_request`, `is_read_only` | Reads the message parts of a request, applies the policy, and returns the changed outputs with one `context` record. |
| `harness-cli/src/workflow.rs` | `system_block`, `state_block`, `short` | The task block for the system prompt. Cleaned and cut short; nothing is created. |
| `opencode-plugin/local-agent.ts` | `applyContextChanges`, two hooks | Forwards both hooks, applies the changes to a copy of each changed message, and fails open. |

## Hook protocol

- `messages-transform`: input `{messages}`, the message list of one request. Output `{changes: [{message, part, output}]}`.
  Fail-open answer: `{}`.
- `system-transform`: input `{sessionID}`. Output `{text}`, which the shim adds to the first system entry. Fail-open
  answer: `{}`.

## Configuration

`[context]`: `target_tokens`, `hard_limit` and `compact_tool_outputs`. See
[configuration](../../guide/configuration.md#context-budget-m6).

## Tests

- `harness-core/src/context.rs`: unit tests for each rule, the budget in tokens, determinism, head and tail, and failure
  lines.
- `harness-cli/src/host/opencode_v1.rs`: three tests of the transform (drops that leave the request within the budget,
  compaction to failure lines, and the switch that turns it off).
- `harness-cli/src/workflow.rs`: the block content and its cleaning, and no block for a session without a task.
- Live: the table in the M6 result.

## Related

- [Architecture](../../concepts/architecture.md)
- [Decision 0006](../../decisions/0006-context-is-a-per-request-view.md)
