# M6: Context management

M6 decides, for each completed tool result, whether the model sees it whole, compacted or dropped, and it puts the
current task state into the system prompt. Both happen per request. The stored OpenCode session is not edited, and no
model decides anything: the rules are deterministic (R-24.2).

> **Status:** Done and committed as `977a56f` (2026-10-26). Gate green (642 tests). The live run is recorded below.

## What the host lets a plugin change

OpenCode 1.19.3 keeps the session. Two experimental plugin hooks change what one model request contains:

- `experimental.chat.messages.transform`: receives the messages of one request and may replace them.
- `experimental.chat.system.transform`: receives the system prompt of one request and may add to it.

Both appear in the pinned `@opencode-ai/plugin` 1.19.3 types, and OpenCode 1.19.3 calls both in the live run (the
`context` records in `events.jsonl`). Both are experimental, so a later OpenCode release may change them.

## Rules

Each completed tool result is planned in this order. The budget comes from `[context]`: `target_tokens` and
`hard_limit`, each multiplied by 4 to get bytes (the estimate; see Limits).

1. A verification result older than the three most recent results (`run_verification`, `run_tests`, `run_linter`,
   `run_typechecker`, or a test command run through `bash`) is compacted to its failure lines.
2. While the context is over the target, older results over 2000 bytes are compacted, oldest first.
3. While the context is over the hard limit, older read-only results are dropped, oldest first. The stub names the
   tool. A write result is never dropped.
4. While the context is over the hard limit, older large results are compacted.
5. While the context is still over the hard limit, large recent results are compacted, oldest first. This rule was
   added after the live run (see Deviations).

A compacted result keeps its first 20 and last 10 lines, with a marker that says how many lines were omitted. A
compacted verification result keeps at most 40 failure lines (lines that contain `fail`, `error`, `assert`, `panick`,
`exception`, `traceback` or `expected`). The three most recent results are kept whole while the context fits.

Nothing is summarised, and no model is consulted.

## Design

- `harness-core/src/context.rs`: the pure policy: `plan`, the two compaction functions and the stub. Unit tests cover
  each rule.
- `harness-cli/src/host/opencode_v1.rs`: `messages_transform` reads the message parts of one request, asks the policy
  for an action per result, and returns only the changed outputs as `{message, part, output}`. It writes one `context`
  record per request.
- `harness-cli/src/workflow.rs`: `system_block` returns the state of the session's task: phase, iteration, last action,
  last test, counters and objective. Control characters are replaced and long fields are cut short, and the block says
  it is information, not an instruction. Nothing is created: a session without a task gets no block.
- `opencode-plugin/local-agent.ts`: forwards both hooks and applies the returned outputs to a copy of each changed
  message. Failure is fail-open: the request keeps its full history, and the system prompt is sent without the block.
  Compaction is not a security control. Outputs are redacted in `tool.execute.after`, before they are stored.
- Configuration: `context.compact_tool_outputs = false` turns compaction and dropping off. Requests are still measured.

## Live run (2026-10-26)

OpenCode 1.19.3 on the toy repository (`D:\e2e\toy`), Qwen3.5-9B on slot `main`. The toy config set
`target_tokens = 1500` (6000 bytes) and `hard_limit = 3000` (12000 bytes) so that a 400-line fixture (27 KB) is over
budget. These are test settings, not the defaults. The task was read-only: read `notes_long.txt`, `calc.py` and
`test_calc.py`, run the unit tests, and report.

| Run | Request | Tool results | Bytes before | Bytes after | Outcome |
|---|---|---|---|---|---|
| A (rules 1 to 4) | 17:47:50 | 3 | 30284 | 30284 | Rejected by the server: 20082 tokens against a 16384-token context. The fixture was one of the three recent results, so nothing changed. |
| B (rules 1 to 5) | 18:01:48 | 3 | 30255 | 2935 | The fixture compacted (1 changed). The run continued. |
| B | 18:02:02 | 3 | 30357 | 3037 | 1 changed |
| B | 18:02:25 | 4 | 30647 | 3327 | 1 changed. The run completed (exit 0). |

Run B has no context or template error in the OpenCode log. Its one tool error came from the model calling `run_tests`
with the step name `run_tests`, which is not a configured step (`unit tests`, `guard`). That is a model mistake, not a
harness one.

The measurements are the `context` records in `.local-agent/events.jsonl`: `tool_results`, `changed`, `bytes_before`,
`bytes_after`, `target_bytes` and `hard_bytes`. Two checks were run by hand with the same payloads.
`agent hook system-transform` returned the task block for the session, and `agent hook messages-transform` returned no
changes for a small request.

Not observed live: rule 1 (an old verification result compacted to its failure lines). It is covered by a unit test
only, because the live task had too few verification results to reach it.

## Deviations from the plan

1. **Rule 5 was added.** The plan kept the three most recent results whole. Run A showed that this can overflow the
   context: a single fresh 27 KB result was one of the three. Large recent results are now compacted, but only when the
   request would not otherwise fit.
2. **The state block joins the first system entry.** A second system message made the Qwen chat template fail with
   "System message must be at the beginning", so the first request after a task was bound failed. The block now joins
   the first entry, and the number of system messages does not change.
3. **The token estimate is low for this content.** Run A's messages were 30284 bytes, and the request was 20082 tokens
   with the system prompt and the tool definitions included. The estimate (bytes divided by 4, messages only) is kept,
   because it is simple and stated. The budget has to leave room for the rest of the request.

## Limits

- The experimental hooks may change between OpenCode releases. Repeat the live run on each upgrade.
- The estimate is bytes over four, over the messages only. It is not measured per tokenizer. Numbered or repetitive
  text takes more tokens per byte than prose, as run A shows. Set `hard_limit` with that in mind.
- A compacted result keeps its head and tail only. A dropped result is replaced by a stub. The model can re-read a file
  to see the middle part.
- No summaries are made. Repository retrieval is the existing code tools (M2); no index is added.
- Rule 1 is not observed live (see above).
- The state block is added to every request of a bound session. It adds about 300 bytes to the system prompt.

## Related

- [M3: the journal and the state](m3-state.md)
- [Decision 0006: context is a per-request view](../../decisions/0006-context-is-a-per-request-view.md)
- [Module: context](../../reference/modules/context.md)
- [Requirements register](../../reference/requirements-traceability.md): the M6 rows
