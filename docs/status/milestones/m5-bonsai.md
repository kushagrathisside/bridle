# M5: The deep model on demand (Ternary Bonsai 2 27B)

M5 makes the deep model usable without giving it the main model's place. It loads on demand, it never runs beside
the main model on the GPU, its reasoning budget is raised, and it is checked and measured like the other slots.
Its escalation routing (R-3.6, R-18.1) was added later, as M5-3 (2026-10-27); see the section below.

> **Status:** Done and committed as `53724d4` (2026-10-26). Gate green (627 tests). Live results below. Escalation routing
> added on 2026-10-27. Its live check passed on 2026-10-28 against the served deep model (M5-3, built).

## Gate and what was checked

| Requirement | Result | Evidence |
|---|---|---|
| Deep slot loads on demand (R-45.3) | Done | `agent models serve deep --detach`: healthy after 27.8 s; `agent models stop deep` releases it |
| One GPU model at a time (R-45.1, R-45.3) | Done | `models serve` refuses to start a GPU slot while another GPU server runs; live refusal named `deep` |
| Reasoning budget raised (R-3.5, plan M5) | Done, with a changed value | `[models.deep.reasoning] budget_tokens` 4096 to 8192 (see Deviations) |
| Model checks pass (R-44.2, AC-B4) | Done | `agent health deep`: 8 of 8, including a tool call and structured output |
| PTQ1_0 file and the PrismML runtime (R-3.3, R-3.2) | Done | the deep slot uses `Ternary-Bonsai-2-27B-PTQ1_0.gguf` on `llama.cpp-prism` b10787 |
| Context (R-3.4) | Done | 16384 (normal deep); the 262K context is not used |
| Bonsai never replaces Qwen (R-3.7, R-18.4) | Done | the deep model answers only the A/B cases; no code path lets it edit |
| Measured against the micro model | Done | the same 20 cases, see below |

## Measurements (2026-10-26, RTX 4060 Laptop, 8 GB)

| State | GPU memory in use |
|---|---|
| Nothing loaded | 187 MiB of 8188 MiB |
| Deep loaded (PTQ1_0, 16K context) | 6552 MiB of 8188 MiB |
| Main (Qwen3.5-9B) loaded, earlier measurement | 5443 MiB of 8188 MiB ([models](../../guide/models.md)) |

The two cannot run together on this card (R-45.1). Deep load time: 27.8 s to healthy.

## A/B: deep against micro on the same 20 cases

| Measure | Micro (LFM2.5-1.2B, CPU) | Deep (Bonsai 2 27B, GPU) |
|---|---|---|
| Model answers valid and allowed | 16 of 16 | 16 of 16 |
| Fallbacks to the rules | 0 | 0 |
| Agreement with the rules, stalled cases | 5 of 16 | 1 of 16 |
| Latency p50 / p95 per stalled decision | 3.2 s / 4.3 s | 2.1 s / 2.9 s |

The deep model chose REPLAN in 10 of 16 stalled cases and ESCALATE or ASK_USER in the rest. The micro model chose
ESCALATE most often. The rules' answers include `NEXT_ACTION DIAGNOSE`, which neither model may choose, so agreement is
low for both. The A/B does not show which answer is right. Reports: `evals/decisions/report-2026-10-26.json` (micro) and
`evals/decisions/report-deep-2026-10-26.json` (deep).

The deep requests disable thinking (`enable_thinking: false`), which is why a stalled decision takes about 2 s; the
health check's completion uses the reasoning budget and takes about 30 s.

## Escalation routing (M5-3, 2026-10-27)

When the decision is ESCALATE and `[routing] enable_deep_escalation = true`, the deep slot is asked for the decision
instead of the person (R-3.6, R-18.1). The rules are as follows, all checked in `workflow::escalate_if_due`:

- An ESCALATE decision is acted on when its confidence reaches `[routing] escalation_threshold`. The rules give a
  repeated failure a confidence that grows with the count of identical failures: 0.5 at two, 0.75 at three (the default
  threshold), 0.875 at four. A counter rule (implementation retries) gives 1.0. The micro model gives none and is always
  acted on; Jev gives its own confidence.
- A task may use `[agent] max_deep_escalations` escalations (default 1). Each attempt counts, successful or not. The
  count is `TaskState::escalation_count`, folded from the journal's `escalated` records. It was never incremented
  before 2026-10-29, so it read 0 in every run, including the runs that escalated.
- The deep call has a timeout of 30 s, not the slot's own (up to 30 min). The shim gives the hook 150 s, and a
  `tool_before` that times out blocks the call, so a slow deep model must not hold the hook. A call that fails or times
  out keeps the escalated decision, which goes to the person.
- When the deep slot runs on the GPU and is not serving, the harness swaps the GPU models for the call: the other GPU
  slots that run are parked, the deep slot starts, and afterwards it stops and the parked slots start again
  (2026-10-28). Without a swap the slot has to be running already (`agent models serve deep`).
- Timing (2026-10-29): a loop rule is evaluated when a verification fails (`after`), not at the agent's next call. An
  escalation taken there is recorded, and its answer is kept in the task until a verification passes or the task is
  reset. The next call is refused with that answer, and the deep model is not asked again. A loop first seen at a call
  (`before`) is still evaluated there.
- Validation: V39 (enable_deep_escalation without a `deep_reasoning` slot) is an error. V40 (`decisions.provider =
  "local_lfm"` while `enable_micro_decisions` is off) is a warning; the micro model is then not used.

Checked by unit tests: the trigger (`escalation_due`), the deep answer replacing the escalated decision, the failure
path keeping it, the cap, the missing deep slot, and that only ESCALATE is escalated. For 2026-10-29: the confidence
curve and the counter rule's confidence (`decision`), the threshold deciding whether the third failure reaches the deep
slot (`workflow`, `the_threshold_decides_...`), a third failing verification escalating at the failure and not again
(`a_third_failing_verification_...`), the kept answer (`the_answer_of_the_deep_slot_...`), the journal's escalation
count and its reset by a pass or a reset (`journal`), and the same-failure count growing past the trigger (`loops`).
The deep model's own answers were measured before (the A/B above). An escalation from a real stalled loop in a live
session is not run; the live check below is.

A live check of the deep answer is in `crates/harness-runtime/src/decision.rs`, the ignored test
`the_deep_slot_answers_an_escalated_loop`. It sends a same-failure escalation to the deep slot and checks that the answer
is one the engine accepts. To run it: stop the main model (`agent models stop main`), serve the deep model
(`agent models serve deep --detach`), then run the test with `--ignored`. `BRIDLE_DEEP_URL` overrides the endpoint.
Without the deep model the test fails with a connection error (checked 2026-10-28), which is the intended result: it
must not pass on a fallback. Run on 2026-10-28 with the deep model served (`ready`, port 8003): passed, the deep answer
was used and its label carried no failure.

## Lifecycle status (R-45.2, 2026-10-28)

`agent models status [slot]` prints one line per slot. The state comes from the PID record and, for a running
process only, one health request (2 s timeout):

- `unloaded`: no record (never started, or stopped).
- `warming`: the process runs and its health endpoint does not answer 200 yet.
- `ready`: the process runs and the health endpoint answers 200.
- `error`: a record exists but its process is gone, or the PID now belongs to another program.

Checked live on 2026-10-28: with no server running, all three slots report `unloaded`. A planted record with a dead PID
reports `error` and names the command that starts the slot again. `ready` and `warming` are covered by unit tests of
the mapping. Live on 2026-10-28, the deep model reported `ready` through the same health check. `warming` is still not seen live.

Not reported: `loaded` (weights resident), `idle` (time since the last request) and `unloading` (a stop in progress).
The command has no signal for them.

## Deviations

- Reasoning budget 8192, not 16384. A 16K context cannot hold a 16K budget plus the prompt and an answer. The
  4096 budget is the measured value that truncated answers; 8192 is the first step above it. A longer budget needs a
  larger context or a shorter prompt, and that is a benchmark (R-3.5) for M10.
- `agent models serve` refuses when another GPU server runs. It does not stop the other one; the person does.

## Not done

- Escalation triggers beyond persistent failure. Only ESCALATE decisions from the rules and the micro model are routed
  (see above). Architecture, research and high-complexity triggers (R-3.6, R-29.1) are not built. Each needs a
  definition of the trigger (a keyword rule, a classifier, or a user mark) that has not been chosen; a deep call on a
  guessed trigger would also swap the GPU model.
- Lifecycle states `loaded`, `idle` and `unloading` (R-45.2) are not reported; see the lifecycle section above. Keeping
  Qwen loaded and unloading it automatically (R-45.3) are not built.
- Large deep (24K to 32K) is not benchmarked (R-3.5). This needs a 32K deep server config and a GPU swap from Qwen, which
  the person runs; nothing was started here.

## Related

- [M4: the micro model](m4-lfm.md)
- [Models and runtimes](../../guide/models.md)
- [Requirements register](../../reference/requirements-traceability.md): the M5 rows
