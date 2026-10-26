# M4: The micro model as a decision engine (LFM2.5-1.2B)

M4 puts the micro model into the decision path for stalled loops. The rules still find the loop, check every limit and
decide every call. The model answers one of three questions when a loop has stalled: stop and report (ESCALATE), ask the
person (ASK_USER), or plan again (REPLAN). The design is in [ADR-0005](../../decisions/0005-micro-model-decides-stalled-loops-only.md).

> **Status:** Done and committed as `49f74d9` (2026-10-26). Gate green (627 tests); live runs recorded below.

## Goal and scope

The gate (AC-O3, AC-O4, R-30) asks for three things: the model's answers are valid or fall back, a stalled loop
is decided by the model when it is available, and an A/B run of the model against the rules passes. Routing between
slots (R-29.1) and the other LFM roles (classification, summarising, subagent routing) are not in M4.

## What was built

| Piece | Where | Test |
|---|---|---|
| The answer contract: one JSON object, `decision` in {ESCALATE, ASK_USER, REPLAN}, fenced JSON allowed, reason capped | `harness-core/src/lfm.rs` | 7 unit tests |
| `Decision::Replan` and `model_reason` on the decision result | `harness-core/src/decision.rs` | the existing decision tests |
| `LfmEngine`: stalled loops only; compact request; strict check; fallback to the rules on any error | `harness-runtime/src/decision.rs` | 5 unit tests with a scripted provider |
| `decide_for`: `decisions.provider = "local_lfm"` selects the micro slot; a failure to build it falls back | `harness-cli/src/workflow.rs` | the black-box fallback test |
| The refusal message shows the model's reason; `agent explain` shows the provider and reason | `harness-cli/src/workflow.rs`, `commands/task.rs` | black-box tests |
| `agent eval-decisions <cases.json> [--out report.json]`: the A/B run | `harness-cli/src/commands/eval.rs` | run live, below |
| 20 recorded cases: 16 stalled loops, 4 limit cases | `evals/decisions/cases.json` | the A/B run |

Black-box tests: 9 in `tests/orchestrator.rs`, including one with the micro endpoint on a closed port, which checks that
the refusal still happens through the rules and that `explain` names the fallback.

## Results

### A/B run (2026-10-26, LFM2.5-1.2B-Thinking on CPU, 8 threads)

| Measure | Value |
|---|---|
| Cases | 20 (16 stalled, 4 limit cases) |
| Model answers | 16 of 16 valid and in the allowed set |
| Fallbacks to the rules | 0 |
| Limit cases decided by the rules | 4 of 4 (the model is never asked) |
| Agreement with the rules, stalled cases | 5 of 16 |
| Latency per stalled decision, p50 / p95 | 3.2 s / 4.3 s |

The full report with each case is in [report-2026-10-26.json](../../../evals/decisions/report-2026-10-26.json).

Reading the agreement figure: the rules answer a repeated command with `NEXT_ACTION DIAGNOSE`, which is not one of the
three answers, so those 5 cases cannot agree by construction. On the repeated-edit cases the model chose REPLAN or ESCALATE
where the rules chose ASK_USER. On the same-failure cases it agreed in 5 of 6. The A/B does not show which answer is
better; it shows that the answers are valid and fast enough to use.

### Live runs

- **Health (8 of 8 checks)** on the micro slot, through `agent health micro`.
- **Hook run with the model** (`task-0011`): four identical `touch note.txt` calls through the real hook. The fourth was
  refused by the repeated-command rule with the model's reason ("Stalled loop indicates unresolved issue requiring
  intervention."), and `agent explain` shows `Escalate because RepeatedCommand (provider local_lfm)`.
- **Edit and re-test cycle with the model in the path** (`task-0008`, `task-0010`): the model did not stall in these
  two runs and no decision was made. The earlier stall in M3 (`task-0007`) showed the rules path. The model's own
  behaviour differs between runs, so this is not a repeatable live test of the whole loop.

## Deviations from the plan and the brief

- `REPLAN` was added to the decision set; the brief's R-20.2 names it.
- The model's reasons are generic. A prompt that asks for a reason tied to the failure would help; it is not changed in M4.
- The A/B cases are recorded, not generated from live runs. They cover the loop shapes the rules produce.

## Limits

- Stalled-loop decisions wait for the model: about 3 seconds on CPU per refusal.
- The model's reasons can be wrong or vague. The decision still goes through the same refusal path, and the reason is
  shown with the decision.
- LFM2.5 is licensed under the LFM Open License v1.0 for research use. The weights are not redistributed (see
  [models](../../guide/models.md)).

## Related

- [ADR-0005](../../decisions/0005-micro-model-decides-stalled-loops-only.md)
- [M3 result](m3-state.md): the rules this builds on
- [Requirements register](../../reference/requirements-traceability.md): the M4 rows
- [Progress log](../progress.md)
