# M5: The deep model on demand (Ternary Bonsai 2 27B)

M5 makes the deep model usable without giving it the main model's place. It loads on demand, it never runs beside
the main model on the GPU, its reasoning budget is raised, and it is checked and measured like the other slots.
Its escalation routing (R-3.6, R-29.1) is not built.

> **Status:** Done and committed as `53724d4` (2026-10-26). Gate green (627 tests). Live results below.

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

## Deviations

- Reasoning budget 8192, not 16384. A 16K context cannot hold a 16K budget plus the prompt and an answer. The
  4096 budget is the measured value that truncated answers; 8192 is the first step above it. A longer budget needs a
  larger context or a shorter prompt, and that is a benchmark (R-3.5) for M10.
- `agent models serve` refuses when another GPU server runs. It does not stop the other one; the person does.

## Not done

- Escalation routing: a stalled loop does not go to the deep model automatically (R-3.6, R-18.1, R-29.1). The engine
  asks only the slot it is configured for. Routing needs the deep model's cost and a policy; it is the next step.
- Lifecycle states (R-45.2) are recorded only as running or stopped by PID.
- Large deep (24K to 32K) is not benchmarked (R-3.5).

## Related

- [M4: the micro model](m4-lfm.md)
- [Models and runtimes](../../guide/models.md)
- [Requirements register](../../reference/requirements-traceability.md): the M5 rows
