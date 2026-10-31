# ADR-0010: Routing between model slots: rules first, a small model only for the cases the rules cannot place

- **Status:** Proposed (2026-10-31)
- **Date:** 2026-10-31
- **Deciders:** Kushagra Srivastava

## Context

Bridle has three slots: `main` (Qwen3.5-9B on the GPU, the daily model), `micro` (LFM2.5-1.2B on the CPU, decides stalled
loops, ADR-0005) and `deep` (Ternary Bonsai 2 27B, on demand, ADR-0007 and M5). Nothing chooses between `main` and
`deep` for a new request; the only escalation is a stalled loop. The architecture doc lists routing between slots as
planned.

The budget is tight, so paid models stay out of routine routing. A cheap classifier is wanted, not a larger model.

LFM3 (released 2026-10-27) has a 1.73B text checkpoint with a 256K context window in Liquid's card (the hosted API caps
at 128K), a conv/attention hybrid of 24 layers, Q4_K_M at about 1.1 GB, and tool calling in an XML form. Its llama.cpp
support needs a build at or above b11960 (v0.8.0), per Liquid's GGUF card. These figures are from the model card and a
third-party guide, not measured here. The license is the LFM Open License v1.0.

## Decision (proposed)

1. **Rules decide first.** A deterministic function maps a request to a slot from visible facts: the task is in a
   stalled loop (already `micro`), the request names a slot, the request is a read-only question, the context is near
   its limit. Most requests are placed here.
2. **A classifier answers only the rest.** When the rules do not place a request, `micro` (or an LFM3 slot if measured
   better) answers one choice from a closed list: `main`, `deep`, or `ASK_USER`. It follows the contract in ADR-0005:
   one JSON object, a choice from the list, anything else rejected and the rules decide.
3. **The classifier sees a summary, not the request text,** the same as the micro model does today. This keeps secrets out
   of a second model.
4. **No slot is chosen by a model without a fallback.** A failed or invalid answer falls back to the rules.
5. **Switching models is measured.** The router is turned on only when the M10 corpus shows that its choices match, or
   beat, the fixed `main` default on the same tasks. The LFM3 slot is added only if it beats LFM2.5 on that corpus.

## Consequences

- One more decision point per request, bounded by the same token limits as the micro model.
- Each routing decision is written to the decision log with its rule code, so a choice can be explained afterwards.
- With one GPU model loaded at a time, a route to `deep` costs a model swap (about 20 s, measured 2026-10-28). The router
  must price that in: a route that swaps the GPU model needs a reason the rules can name.

## Result of the measured comparison (2026-10-31)

- **LFM3-1.7B does not replace LFM2.5-1.2B as the micro model.** On the 20 recorded loop cases it answers ASK_USER for all
  16 stalled cases. Its five agreements with the rules are the repeated-edit cases where the rules also say ASK_USER.
  LFM2.5 varies its answers and agrees on five different cases. Details and reports are in
  [progress.md](../status/progress.md).
- **The routing thresholds cannot be set yet.** The recorded corpus covers stalled loops only. It has no cases for choosing
  between `main` and `deep`, so there is no data to set a threshold from. Setting one without it would be guessing.
- **A router that depends on the small model's choices is not supported by this data.** The small model agrees with the
  rules on 5 of 16 cases, and the two models disagree with each other on the rest.

## Open

- A routing corpus of recorded requests, labelled with the slot that should answer each. A seed corpus of 20 requests is in
  `evals/routing/cases.json`, and the command `agent eval-routing` measures it. The seed labels must be reviewed by the owner; they overlap the rule keywords, so they do not yet show how the placement generalises.
  The corpus has to grow from real requests before any threshold is set.
- The measurements (progress.md): rules only, 12 of 20; with the decision model, 19 of 20.
- Whether a prompt change would make LFM3 vary its answers. That is a separate experiment and is not claimed here.
