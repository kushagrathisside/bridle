# M10: evaluation

M10 is the benchmark: a corpus of tasks, two conditions, a runner and a report (R-39, R-40, R-57.10). The design is in the
`agent bench` command (`crates/harness-cli/src/commands/bench.rs`) and the corpus under `evals/`.

> **Status:** Built, and the full corpus has run once under both conditions (2026-10-27, one run per task). Minimal
> passed 6 of 6; harness passed 4 of 6. The sample is six tasks and one run each: a lead, not a measured effect. The
> start stall that blocked the earlier runs is explained and fixed (see below).

## What is built

- **Runner** (`agent bench run --condition <file> --opencode <exe> --out <dir>`): copies a task's fixture into a fresh folder,
  writes the condition's config, runs `agent init` there so OpenCode is wired to the harness, starts OpenCode with the task
  prompt and the shared protocol sentence, and checks the result with the task's `verify.py`. A run that never reaches the
  model within five minutes is restarted, up to three times; each restart is counted in the result.
- **Report** (`agent bench report <folder>...`): one row per condition, one row per task, and the caveat about sample size.
- **Corpus** (`evals/tasks/`): six tasks in six categories (debugging, test repair, implementation, repository comprehension,
  refactoring, experiment analysis). Each has a fixture, a prompt, and a `verify.py` that lives outside the run folder.
- **Conditions** (`evals/bench/conditions/`): `minimal` (policy only: workflow off, compaction off) and `harness` (workflow on,
  compaction on).

## Checks on the corpus

Each `verify.py` was run on its untouched fixture and on a reference fix. Every one fails on the fixture and passes on the
reference (2026-10-27). This shows the checks can fail and can pass; it does not show that a model passes them.

## Why the starts stalled (2026-10-27)

OpenCode 1.19.3 starts a background dependency install for the project's `.opencode` folder when that folder holds a
plugin. The start waits for it. The benchmark removes the proxy variables from OpenCode's environment, so the install
reached the package registry directly and never finished. Measured with one task, the same folder, and only the
environment changed:

| Setup | Start (creating instance to event connected) |
|---|---|
| No plugin file | about 0.5 s |
| Trivial plugin, proxy variables inherited | about 16 s |
| The real plugin, proxy variables removed, registry as normal | no progress for more than 5 minutes (three attempts); one isolated run resumed after about 2.5 minutes |
| Trivial plugin, proxy removed, registry pointed at a closed loopback port | about 71 s, then "background dependency install failed" and the start goes on |
| The real plugin, registry pointed at a closed loopback port (the fix below) | about 71 s, then the model is reached |

Not the cause: the per-run session database, the isolated XDG directories, the startup switches, an empty
`.opencode/package.json`, and the MCP server (it answers `initialize` in about 30 ms).

The fix is in `start_opencode`: `NPM_CONFIG_REGISTRY` and `BUN_CONFIG_REGISTRY` point at `http://127.0.0.1:9/`, so the
install fails after a bounded wait and nothing leaves the machine. The cost is about 70 s per start. A run that is
still stalled after five minutes is restarted, as before.

Alternative, not taken: let OpenCode use the proxy for its start. It is faster (about 16 s), but it sends OpenCode's
start-up traffic out through the proxy. That is an egress choice for the person, not one the benchmark makes by itself.

## Live runs

| Run | Result |
|---|---|
| First one-task runs (before the fixes) | OpenCode stalled in every attempt, with no model request. |
| One task, default data folder, after the runner fixes | Ran to the end: 13 tool calls, 6 blocked, verify failed (the model asked the user which option to take instead of finishing). The protocol sentence was added after this. |
| Full corpus, both conditions (start-up switches on) | Stopped by the user before it finished. Minimal condition: 4 of 6 tasks ended (one passed, `implement-fizzbuzz`; `analyse-latency` stalled in all three attempts). |
| Full corpus, both conditions, after the registry fix (2026-10-27, `m10-full-minimal`, `m10-full-harness`) | Minimal: 6 of 6 passed. Harness: 4 of 6 passed. No timeouts, no restarts. Results: `evals/bench/results/2026-10-27/`. |

Start-up switches were turned on for benchmark runs (`OPENCODE_DISABLE_MODELS_FETCH`, `OPENCODE_DISABLE_AUTOUPDATE`,
`OPENCODE_DISABLE_DEFAULT_PLUGINS`, `OPENCODE_DISABLE_LSP_DOWNLOAD`). They did not stop the stalls. The shared database is
the current suspect; the per-run database (`OPENCODE_DB`) is in the runner and type-checks, but has not been run through the
full corpus yet.

## Full run (2026-10-27)

One run per task, both conditions, the dev-target binary built from the code in this commit, the registry fix in place.

| Task | Category | Minimal | Harness | Tool calls (minimal / harness) |
|---|---|---|---|---|
| analyse-latency | experiment analysis | pass | pass | 2 / 3 |
| comprehend-stock-path | repository comprehension | pass | fail | 9 / 2 |
| debug-range-sum | debugging | pass | fail | 4 / 2 |
| implement-fizzbuzz | implementation | pass | pass | 4 / 4 |
| refactor-rename | refactoring | pass | pass | 10 / 10 |
| repair-wrong-test | test repair | pass | pass | 4 / 4 |

Totals: minimal 6 of 6, harness 4 of 6. Mean time 185 s (minimal) and 189 s (harness). Mean context 8734 bytes
(minimal) and 5293 bytes (harness). Both harness failures ended after two tool calls, each with one blocked call. The event
log of both runs records a permission request that was replied to and no further tool call; in one of the two, the session
then went idle. A benchmark run has no person to approve an `ask`, so the request is refused (decision D3). The minimal
runs on the same two tasks had blocked calls too and went on. With one run per task this is a lead, not a finding.
Checking it needs repeated runs and the text of the blocked call, which the event log does not hold.

The earlier full run (before the registry fix) reported minimal 1 of 6 and harness 0 of 6. Those runs stalled before the
model; they are not comparable with this run and are not reported as results.

## Repeat run (2026-10-28)

The same six tasks, both conditions, run again on the same main model (Qwen3.5 9B, served on port 8001), so the two
full runs can be compared. Results: `evals/bench/results/2026-10-28/` (`minimal/`, `harness/`, `report.md`).

| Task | Minimal | Harness | Tool calls (minimal / harness) |
|---|---|---|---|
| analyse-latency | fail | pass | 4 / 27 |
| comprehend-stock-path | fail | pass | 3 / 8 |
| debug-range-sum | fail | pass | 3 / 6 |
| implement-fizzbuzz | pass | fail | 4 / 19 |
| refactor-rename | pass | pass | 10 / 11 |
| repair-wrong-test | pass | pass | 4 / 4 |

Totals: minimal 3 of 6, harness 5 of 6. Across both full runs, the totals are minimal 9 of 12 and harness 9 of 12. Per-task results change between runs; the next section explains why this sample cannot settle that.

Findings from the repeat run, read from the run logs:

- **Minimal failures came from a policy block on a harmless command.** In all three minimal failures the model ran an `awk`
  command whose `$2` looked like a shell expansion. The path check refused it as an unresolved path
  (`PRIVILEGED, unresolved_path`). The model then could not write the result file that the verify script reads. The policy did what it is written to do, but
  this is a false positive on a plain `awk` field reference. Fixing it changes the policy, so it is not changed here.
- **The harness fizzbuzz failure came from a false positive and a retry limit.** The path check refused a `*.py` glob with
  "path escapes the workspace through a symlink or junction". No symlink or junction exists in that folder (checked). The
  model then hit the `implementation_retries` loop limit and stopped, with the test still failing. The model also built
  one shell command with backslashes removed, which the path check refused as an alternate data stream.
- **Harness used more tool calls and spent more of them blocked.** Mean 12.5 calls and 4.8 blocked, against 4.7 and 0.8 for
  minimal. Mean wall time 257 s (harness) and 171 s (minimal). Mean context 44640 bytes (harness) and 6346 bytes (minimal).

The sample is still six tasks, two runs each. The two conditions have the same pass count across both runs (9 of 12 each),
so these runs do not separate them.

## Condition D: policy decision (2026-10-28)

Condition D is Qwen plus LFM plus Bonsai. One GPU model runs at a time (R-45.1), so Bonsai cannot sit beside Qwen on the GPU.
Two ways to run D were timed on the same escalation request (151 to 236 prompt tokens, 28 output tokens, the length of a
decision answer):

| Approach | Measured | Per escalation |
|---|---|---|
| Bonsai on CPU, Qwen stays on GPU | 48.1 s (236 prompt tokens at 8.1 tok/s, 28 output at 1.43 tok/s) | about 48 s |
| GPU swap: stop Qwen, serve Bonsai, answer, stop Bonsai, serve Qwen | stop Qwen 1 s; Bonsai healthy 11.1 s; answer 1.8 s (33.5 tok/s); stop Bonsai 0.4 s; Qwen healthy 6.0 s | about 20 s |

Decision: the GPU swap is the faster approach, about 2.4 times faster per escalation, so D uses the swap.

Two changes are needed before D can run as a task condition. Neither is in the code yet:

- The swap cannot run inside the `tool_before` hook. The shim gives each hook 20 s (`HOOK_TIMEOUT_MS` in `opencode-plugin/local-agent.ts`), and a timed-out `tool_before` fails closed, which blocks the tool call. The hook limit must be raised to at least 30 s for an escalation, or the swap must run outside the hook.
- The deep call is capped at 15 s (`ESCALATION_TIMEOUT_MS`). A swap-based escalation needs about 2 s for the answer and about 20 s for the swap, so the cap must be raised.

Bonsai on CPU is not used. Its speed on this machine makes every escalation slower than the 15 s cap, so it would fall back to the
rules every time.

The swap is built (`with_deep_on_gpu` in `crates/harness-cli/src/workflow.rs`). The deep slot starts on the GPU when a GPU
deep call is due, the other GPU slots are parked, and afterwards they start again. The deep call cap is now 30 s, and the shim's
hook limit is 150 s.

Checks run on 2026-10-28:

- Live swap test `a_deep_escalation_swaps_the_gpu_model_and_restores_it` (ignored; run by hand): passed in 15.0 s. Qwen was
  parked, Bonsai reported healthy, Qwen restarted, and Bonsai was stopped.
- Condition D (`evals/bench/conditions/deep.toml`) on one task, `analyse-latency`: passed in 196 s with 2 tool calls. No escalation
  happened in that run, so the swap was not exercised by the task. This timing is an ordinary task, not a swap timing.

D still has no task-level result with an escalation. Each escalation costs about 17 to 20 s: the swap (15 s measured) plus
the deep answer (about 2 s measured). The task-level cost needs a task where an escalation actually fires.

## What remains

- Corpus size. Six tasks is too few to separate the conditions. More tasks need choosing, each with a verify script. That
  choice is yours.
- Tokens, GPU memory and first-pass success are not measured. Those are open (R-39.4).
- The path-check false positives above (`awk` with `$n`; a `*.py` glob) need a decision on the policy before the next run.
- The per-run start includes about 70 s waiting for the blocked registry. The benchmark time includes that wait.
- A decision for the person: letting OpenCode use the proxy for its start (faster, about 16 s, but start-up traffic
  leaves the machine). Not taken; see the stall section.

## Related

- [Checklist](../checklist.md): rows M10-1 to M10-3
- [Requirements register](../../reference/requirements-traceability.md)
