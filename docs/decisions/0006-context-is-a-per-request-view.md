# ADR-0006: Context compaction is a per-request view, decided by rules

- **Status:** Accepted (2026-10-26, with the M6 result)
- **Date:** 2026-10-26
- **Deciders:** Kushagra Srivastava

## Context

The local models have a 16384-token context. OpenCode keeps the whole session and sends it on each request, so long tool
outputs fill the context and the request is rejected. The brief asks for context management with rules first (R-24.2),
and for the task state to be kept apart from the transcript (R-23.3). OpenCode 1.19.3 offers two experimental hooks
that change one request and leave the stored session alone.

## Decision

1. Compaction changes the request, not the stored session. The transforms run per request, and the next request starts
   again from the stored history.
2. Rules only. The policy is deterministic: age, size, kind and the budget. No model decides what is kept (R-24.2).
3. The three most recent results are kept whole while the context fits. They are compacted only when the context would
   otherwise not fit the hard limit.
4. Both transforms fail open. A failure sends the request unchanged. Redaction stays in `tool.execute.after`, so
   compaction never decides what is secret.
5. The task state is information, not an instruction. It is cleaned and cut short, and it joins the first system entry.

## Consequences

- The model sees the head and tail of a long result, or a stub for an old read. It can re-read a file for the middle.
- Each request records the bytes before and after, so the effect is visible on every upgrade.
- The design depends on two experimental hooks. If an OpenCode upgrade removes them, the sessions grow back to the full
  history, and the live run must be repeated.
- The budget is an estimate in bytes, not a tokenizer count.

## Alternatives considered

- Rewriting the stored session with a summary. Rejected: it needs a model to summarise (R-24.2), and it changes what the
  user's session contains.
- Keeping the three most recent results whole regardless of size. Rejected after run A overflowed the context.

## References

- [M6 result](../status/milestones/m6-context.md)
- [Architecture](../concepts/architecture.md)
