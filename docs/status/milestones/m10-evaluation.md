# M10: evaluation

M10 is the benchmark: a corpus of tasks, two conditions, a runner and a report (R-39, R-40, R-57.10). The design is in the
`agent bench` command (`crates/harness-cli/src/commands/bench.rs`) and the corpus under `evals/`.

> **Status:** Built, with the live results not yet recorded. The runner, the report and the corpus are built and checked.
> The live runs stalled: OpenCode sometimes never reached the model. The first cause found was a shared OpenCode session
> database, and each run now gets its own. That fix is not yet verified in the runner, so no benchmark numbers are claimed.

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

## Live runs

| Run | Result |
|---|---|
| First one-task runs (before the fixes) | OpenCode stalled in every attempt, with no model request. |
| One task, default data folder, after the runner fixes | Ran to the end: 13 tool calls, 6 blocked, verify failed (the model asked the user which option to take instead of finishing). The protocol sentence was added after this. |
| Full corpus, both conditions (start-up switches on) | Stopped by the user before it finished. Minimal condition: 4 of 6 tasks ended (one passed, `implement-fizzbuzz`; `analyse-latency` stalled in all three attempts). |

Start-up switches were turned on for benchmark runs (`OPENCODE_DISABLE_MODELS_FETCH`, `OPENCODE_DISABLE_AUTOUPDATE`,
`OPENCODE_DISABLE_DEFAULT_PLUGINS`, `OPENCODE_DISABLE_LSP_DOWNLOAD`). They did not stop the stalls. The shared database is
the current suspect; the per-run database (`OPENCODE_DB`) is in the runner and type-checks, but has not been run through the
full corpus yet.

## What remains

- A full corpus run under both conditions with the per-run database, and the results recorded here.
- Tokens, GPU memory and first-pass success are not measured. Those are open (R-39.4).
- The sample is six tasks, one run each. A difference is a lead, not a measured effect.

## Related

- [Checklist](../checklist.md): rows M10-1 to M10-3
- [Requirements register](../../reference/requirements-traceability.md)
