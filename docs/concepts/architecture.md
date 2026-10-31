# Architecture

This page describes how Bridle is put together: its components, how a tool call passes through them, where state is
kept, and which tools the MCP server offers. Read it before you trust the harness with a repository. It assumes you know
what OpenCode and MCP are.

## Overview

Bridle runs a local model inside OpenCode and puts deterministic checks around it. Every tool call is judged by a policy.
Verification runs real commands. Secret values are redacted before the model sees them, and each decision is recorded in
a log. The model is not trusted to decide what is allowed.

## Components

| Component | Location | Responsibility |
|---|---|---|
| OpenCode 1.19.3 | External program (`opencode.exe`) | Sessions, the agent loop, the interface and the model requests. |
| Shim | `opencode-plugin/local-agent.ts`, copied to `.opencode/plugins/` | Forwards OpenCode hook payloads to `agent hook` and applies the answer. It holds no policy logic. |
| CLI | `crates/harness-cli`, binary `agent` | Commands, hook entry points, the OpenCode adapter (`host/opencode_v1.rs`) and the MCP entry point. |
| Runtime | `crates/harness-runtime` | Model providers and health checks, the server launcher, the MCP server and its tools, the task state store and the event log. |
| Core | `crates/harness-core` | Configuration and validation, the policy, command classification, redaction, loop detection, the decision rules and the task model. |
| Model servers | llama.cpp `llama-server` | OpenAI-compatible endpoints on `127.0.0.1`, one per model slot. |

Two boundaries matter:

- The core crate has no network dependencies and no knowledge of OpenCode.
- The `host` module of the CLI is the only code that reads OpenCode's payloads. A new OpenCode version therefore needs
  changes in that one module. See [ADR-0001](../decisions/0001-rust-core-thin-host-adapter.md).

## Model slots

| Slot | Model | Where it runs | Used for |
|---|---|---|---|
| `main` | Qwen3.5-9B | GPU | The agent's own turns. |
| `micro` | LFM2.5-1.2B-Thinking | CPU | Decisions on stalled loops, when enabled. |
| `deep` | Ternary Bonsai 2 27B | GPU, on demand | Escalation, when enabled. The `main` server is stopped while `deep` runs, then restarted. |

Only one GPU model runs at a time. See [serve the models](../guide/models.md).

## A tool call, step by step

1. The model asks OpenCode to run a tool: a built-in tool such as `bash`, `read` or `edit`, or an MCP tool named
   `local-agent_<name>`.
2. OpenCode calls the shim's `tool.execute.before` hook. The shim sends the call to `agent hook tool-before`.
3. The CLI turns the call into a request and asks the policy for a verdict. The policy checks the paths, the secret
   files, the shell arguments and the command class.
4. The shim throws unless the verdict is `allow`. OpenCode returns the message to the model. If the hook cannot answer,
   the call is blocked. The hook fails closed.
5. With the workflow enabled, the call is recorded in the task journal, and the loop rules can refuse it. See
   [tasks and loops](tasks-and-loops.md).
6. After the tool runs, `tool.execute.after` sends the output to `agent hook tool-after`, which redacts secret values.
   If redaction cannot run, the shim replaces the output with a notice and withholds it.
7. Every decision is written to `.local-agent/tool-events.jsonl`, with secret values left out.

Three other hooks run outside a tool call:

- `experimental.chat.messages.transform` runs before each model request. It may shorten older tool results in that
  request only. If it fails, the request keeps its full history.
- `experimental.chat.system.transform` adds the task state to the system prompt. If it fails, the prompt is sent without
  the task state.
- `permission.ask` asks the policy for a permission decision. If the policy cannot answer, the permission is denied.

Shell environment variables come from the `shell.env` hook. Running the command is still checked at step 2. The `event`
hook forwards lifecycle events for telemetry only.

## Where state lives

| Location | Contents |
|---|---|
| The repository | `local-agent.toml` (project configuration), `opencode.json` (generated), `.opencode/plugins/local-agent.ts` (the shim) and a `.gitignore`. All are written by `agent init`. |
| `.local-agent/` in the repository, ignored by git | `tool-events.jsonl` (tool decisions), `events.jsonl` (the event log), `generated.json` (a record of the generated files), `logs/` (server logs), `servers/` (process records) and `state.json` (the index of sessions, tasks and worktrees). |
| `.local-agent/mcp-results.jsonl` | Verification results recorded by the MCP server. |
| `.local-agent/tasks/<task_id>/` | `task-events.jsonl` (the journal, which is the source of truth), `task.json` (a cache of the state folded from the journal), `decisions.jsonl` and `lock`. |
| `.worktrees/agent-task-NNN/` | The worktree of a task started with `agent task start --worktree`. |
| The machine | `%APPDATA%\local-agent\config.toml`, written by `agent init --machine`. It records the model and runtime folders. |

The model and runtime folders are outside the repository, and the machine configuration sets them.

## MCP tools

`agent mcp` serves JSON-RPC over standard input and output. OpenCode registers each tool as `local-agent_<name>`. The
policy classes each tool by name. A tool name the policy does not recognise is treated as EXECUTE_RISKY and is refused
under the default settings.

| Class | Tools | Notes |
|---|---|---|
| READ_ONLY | `git_status`, `git_diff`, `git_history`, `git_log_range`, `harness_info`, `inspect_config`, `repo_map`, `file_summary`, `symbol_search`, `find_references`, `dependency_search`, `parse_test_failure`, `inspect_artifact`, `summarize_experiment`, `compare_experiments` | Allowed. `find_references` and `symbol_search` read source files with tree-sitter. |
| EXECUTE_SAFE | `run_verification`, `run_tests`, `run_linter`, `run_typechecker`, `run_command` | Each command is checked by the policy inside the tool. `run_verification` runs only the steps in the project configuration. |
| Configured | `adapter_<name>`, from `[[tools.adapters]]` in the machine configuration | Runs a fixed program with the same checks as `run_command`. |

Two optional tools are offered only when turned on in the configuration: `task_status` (`[workflow] task_status = true`)
and `subagent` (`[subagents] enabled = true`). The policy does not yet list either one in its classification tables.

The MCP server redacts the text it returns. It records verification results before redaction, so their fingerprints come
from the raw output.

## Limits

- This release runs on native Windows and is verified on one machine. See the [project status](../project/status.md).
- The harness is written for OpenCode 1.19.3. A different version needs its own adapter.
- The code tools parse files on demand. There is no persistent code index.
- The policy is not a sandbox. See [security](security.md).

## Related

- [Security](security.md): the policy rules and the residual risks.
- [Tasks and loops](tasks-and-loops.md): the task journal and the loop rules.
- [Configuration reference](../guide/configuration.md): the settings and the rules for each file.
- [Module documentation](../reference/modules/README.md): code-level pages for each crate.
- [Decisions](../decisions/0001-rust-core-thin-host-adapter.md): the recorded architecture decisions, starting with ADR-0001.
