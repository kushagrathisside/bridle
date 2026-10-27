# M10: evaluation

M10 is the benchmark: a corpus of tasks, two conditions, a runner and a report (R-39, R-40, R-57.10). The design is in the
`agent bench` command (`crates/harness-cli/src/commands/bench.rs`) and the corpus under `evals/`.

> **Status:** Built and checked. The live stall is explained and fixed in the runner (2026-10-27): a start waits for
> OpenCode's dependency install, which can only finish through the network. The corpus results are not recorded yet; the
> full run is in the Live runs section below.

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
