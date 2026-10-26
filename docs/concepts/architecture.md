# Architecture

This page describes how Bridle is built: its layers, the path a tool call takes, where state is stored, and which
MCP tools it serves. It is for engineers who want to know which component decides what before they trust the
system with a repository. It assumes you know what OpenCode is and what an MCP server does.

> **Status:** release 1 (native Windows). Milestones M0 to M6 are done and verified on one machine; M7 is built and tested, not live; M3 to M5 were run live
> with the local models on 2026-10-26; M6 (context management) is built and verified live (see [progress.md](../status/progress.md)). Everything under
> [Planned components](#planned-components) is not built yet.

## Overview

Bridle is a local-first coding harness. It runs a small local model (Qwen3.5-9B on the GPU, in slot `main`) inside
OpenCode, and puts deterministic checks around it: a policy on every tool call, verification by real commands,
redaction of secrets, and an event log. The model is not trusted to decide what is allowed.

The project has three layers:

| Layer | State | What it covers |
|---|---|---|
| Harness | Built (M0 to M2) | Policy on every tool call, deterministic tools, redaction, decision log; runs inside OpenCode 1.19.3 |
| Orchestrator | Built (M3 to M5): task state, loop detection, rules decisions, the micro model for stalled loops, and the deep model on demand. Routing is planned (see [Planned components](#planned-components)) |
| Local-first application | Goal | A full application on the developer's machine, using local models |

Model slots:

| Slot | Model | State |
|---|---|---|
| `main` | Qwen3.5-9B | In use |
| `micro` | LFM2.5-1.2B | Built (M4): stalled-loop decisions on CPU |
| `deep` | Ternary Bonsai 2 27B | Built (M5): on demand, one GPU model at a time |

## Components

| Component | Where | What it does |
|---|---|---|
| Host | OpenCode 1.19.3 (`opencode.exe`) | Sessions, the agent loop, the UI |
| Shim | `opencode-plugin/local-agent.ts` | Forwards hook payloads to `agent hook`; applies the verdict; no policy logic |
| CLI | `crates/harness-cli` (binary `agent`) | Commands, the OpenCode adapter (`host/opencode_v1.rs`), hooks, MCP entry point |
| Runtime | `crates/harness-runtime` | Model providers, health checks, server launcher, host probes, event log, MCP server and tools |
| Core | `crates/harness-core` | Config and validation, model registry, path guard, command classifier, redaction, tool policy, telemetry types |
| Models | llama.cpp `llama-server` (GPU) | OpenAI-compatible endpoints on `127.0.0.1` |

Boundaries:

- The core crate has no network code and no OpenCode knowledge.
- The CLI's `host` module is the only code that knows OpenCode's payloads (decision D-1 and
  [ADR-0001](../decisions/0001-rust-core-thin-host-adapter.md)). A new OpenCode version means a new adapter module,
  not edits across the code.

## Request flow for a tool call

1. The model asks OpenCode to run a tool (for example `bash`, `read`, `edit`, or a `local-agent_*` MCP tool).
2. OpenCode calls the shim's `tool.execute.before`. The shim runs `agent hook tool-before` with the payload on stdin.
3. The CLI maps the call to a `ToolRequest` and calls `ToolPolicy::evaluate`. That checks the paths, secret files,
   shell arguments (`command_risk` plus the argument check), and the verdict table.
4. Only an Allow passes. Ask and Deny are both blocked (decision D3). The shim throws, and OpenCode shows the reason
   to the model.
5. After the tool runs, `tool.execute.after` redacts secret values from the output before the model sees it. MCP
   results are redacted by the MCP server itself, because OpenCode does not pass MCP output to this hook.
6. Every decision is written to `.local-agent/tool-events.jsonl`, with secret values kept out. With the workflow enabled,
   the call is also recorded in the task's journal, and a loop rule can refuse it (see below).

Policy hooks fail closed. If `agent hook` cannot answer (bad config, missing binary), the shim blocks the call.

Two more hooks run before each model request, not around a tool call. `messages-transform` may shorten old tool results in
that request only, and `system-transform` adds the task state to the system prompt. Both fail open, and neither changes a
verdict. See [M6](../status/milestones/m6-context.md).

The rules applied in step 3 are listed in [security.md](security.md) and, at code level, in
[core-security.md](../reference/modules/core-security.md). The hook protocol is in
[cli-host-opencode.md](../reference/modules/cli-host-opencode.md).

## Where state lives

| Location | Files |
|---|---|
| The repository | `local-agent.toml` (project config), `opencode.json` (generated), `.opencode/plugins/local-agent.ts` (shim copy) |
| The repository's `.local-agent/` (gitignored) | `tool-events.jsonl`, `events.jsonl`, `generated.json` (hashes of generated files), `logs/` (server logs), `servers/` (PID records) |
| `.local-agent/state.json` | Session-to-task index, next task number, pending task, worktrees, claimed MCP results (M3) |
| `.local-agent/mcp-results.jsonl` | Verification results recorded by the MCP server, claimed by the after-hook (M3) |
| `.local-agent/tasks/<task_id>/` | `task-events.jsonl` (the journal, the source of truth), `task.json` (a cache of the folded state), `decisions.jsonl`, `lock` (M3) |
| `.worktrees/agent-task-NNN/` | Worktree of a task started with `agent task start --worktree`, with its own `.local-agent/` (M3) |
| The machine | `%APPDATA%\local-agent\config.toml` (model and runtime locations; written by `agent init --machine`) |
| Drive `D:` | Model weights and runtimes, only (decision D-2). Nothing large is written to `C:`. |

## MCP tools

`agent mcp` serves newline-delimited JSON-RPC over stdio. OpenCode registers each tool as `local-agent_<name>`. The
tool policy classes each tool by name (rule `harness_tool`); an unrecognised tool name is EXECUTE_RISKY (rule
`unknown_tool`) and is blocked.

| Tool | Class | Milestone |
|---|---|---|
| `git_status`, `git_diff`, `harness_info` | READ_ONLY | M1 |
| `run_verification` | EXECUTE_SAFE; each step is classified inside the tool | M1 |
| `git_history`, `git_log_range` | READ_ONLY | M2 |
| `repo_map`, `file_summary`, `symbol_search`, `find_references` (tree-sitter, [ADR-0003](../decisions/0003-tree-sitter-for-code-intelligence.md)) | READ_ONLY | M2 |
| `dependency_search`, `parse_test_failure`, `inspect_config` | READ_ONLY | M2 |
| `inspect_artifact`, `summarize_experiment`, `compare_experiments` | READ_ONLY | M2 |
| `run_tests`, `run_linter`, `run_typechecker`, `run_command` | EXECUTE_SAFE; each command is checked inside the tool | M2 |
| `adapter_<name>` (configured in `[[tools.adapters]]`) | EXECUTE_SAFE verdict from config; the command is checked inside the tool | M2 |

`run_verification` and the `run_tests`, `run_linter` and `run_typechecker` tools run only the steps listed in the
project config. Each step is classified by the same command policy, so a risky step is blocked, not run.
`run_command` and adapters go through the full policy (secret and path checks, command class) before they run.
Details are in [m2-tools.md](../status/milestones/m2-tools.md).

## Planned components

| Component | Milestone |
|---|---|
| Cached symbol index; plan and summary files for tasks | later (M3 uses the code index on demand) |
| Routing between model slots (R-29.1); LFM classification and summarising | M4 partly, later |
| Escalation routing to the deep model (R-3.6, R-29.1) | later |
| LiteLLM | M8 |
| WSL runtime | Release 2 ([ADR-0002](../decisions/0002-runtime-os-repo-location-and-deviation.md)) |

## Limits

- Release 1 runs on native Windows only, and is verified on one machine.
- The harness depends on OpenCode 1.19.3. Another version needs a new adapter module.
- The policy is not a sandbox; see [security.md](security.md#residual-risks).

## Related

- [security.md](security.md): the security model, policy rules and residual risks.
- [installation.md](../guide/installation.md): installing the harness on Windows.
- [configuration.md](../guide/configuration.md): config files, profiles and which file may set what.
- [models.md](../guide/models.md): model slots and serving.
- [Module documentation](../reference/modules/README.md): code-level pages for each crate.
- [progress.md](../status/progress.md): what is verified, with evidence.
- [ADR-0001](../decisions/0001-rust-core-thin-host-adapter.md): Rust core and thin host adapter.
- [requirements-traceability.md](../reference/requirements-traceability.md): requirements and directives D-1 to D-5.
