# ADR-0005: The micro model decides stalled loops only, and only among three answers

- **Status:** Accepted (2026-10-26)
- **Date:** 2026-10-26
- **Deciders:** Kushagra Srivastava

## Context

The design calls for a local model that decides where the rules cannot, and it forbids letting that model
become a controller. The loop rules already name a stalled loop (same failure, repeated command, repeated
edit). The iteration and retry limits are fixed in advance. The micro model is LFM2.5-1.2B on the CPU, which is
small enough to be wrong and fast enough to be used on each stalled loop.

## Decision

1. The rules run first. The model is consulted only when the rules report a stalled loop. Limits (iteration and
   implementation retries) are never sent to the model.
2. The model may answer one of three values: `ESCALATE`, `ASK_USER` or `REPLAN`. The answer must be one JSON object
   with a `decision` from that set. Anything else is rejected, and the rules decide.
3. The model sees a compact summary: the phase, the counters, the last action (capped) and the names of the signals. It
   does not see the prompt, command text or tool output.
4. The model's answer changes the decision that is recorded and shown. It never allows or denies a call on its own: a
   call is refused only by a loop rule, and a read-only call is never refused.
5. Selection is by configuration: `decisions.provider = "local_lfm"`. The default stays `rules`.

## Consequences

### Positive

- The harness keeps its guarantees: the policy decides every call, and the limits are deterministic.
- An unreachable or wrong model costs a refusal decided by the rules, not a session.
- The answers can be audited: the decision, the provider and the model's reason are in `decisions.jsonl`.

### Negative

- The model's reasons are short and generic in the A/B run; they explain little.
- On the A/B cases the model agreed with the rules in 5 of 16 stalled cases. Some of that is structural (the rules'
  `NEXT_ACTION DIAGNOSE` is not one of the three answers). The rest is a real difference, and this run cannot say which
  answer is better.
- Each stalled decision waits for the model: about 3 seconds at the median on CPU.

## Alternatives considered

- **The model decides every loop and phase.** Rejected: it would decide calls, not only stalls, and the rules would no
  longer be the reference for the A/B.
- **The model writes the refusal text only.** Rejected: the decision would then be the rules', and the model would have
  no effect on what the harness does.
