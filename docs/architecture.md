# Architecture

Status: release 1 (Windows native), M1 in progress. See [progress.md](progress.md) for what is verified.

## What it is

A local-first coding harness. It runs a small local model (Qwen3.5-9B on the GPU) inside OpenCode, and
puts deterministic checks around it: a policy on every tool call, verification by real commands, redaction
of secrets, and an event log. The model is not trusted to decide what is allowed.

## Layers

| Layer | Where | What it does |
|---|---|---|
| Host | OpenCode 1.19.3 (`opencode.exe`) | Sessions, the agent loop, the UI |
| Shim | `opencode-plugin/local-agent.ts` | Forwards hook payloads to `agent hook`; applies the verdict; no policy logic |
| CLI | `crates/harness-cli` (binary `agent`) | Commands, the OpenCode adapter (`host/opencode_v1.rs`), hooks, MCP entry point |
| Runtime | `crates/harness-runtime` | Model providers, health checks, server launcher, host probes, event log, MCP server and tools |
| Core | `crates/harness-core` | Config and validation, model registry, path guard, command classifier, redaction, tool policy, telemetry types |
| Models | llama.cpp `llama-server` (GPU) | OpenAI-compatible endpoints on `127.0.0.1` |

The core crate has no network code and no OpenCode knowledge. The CLI's `host` module is the only code that
knows OpenCode's payloads (decision D-1 and ADR-0001).

## Request flow (a tool call)

1. The model asks OpenCode to run a tool (for example `bash`, `read`, `edit`, or a `local-agent_*` MCP tool).
2. OpenCode calls the shim's `tool.execute.before`. The shim runs `agent hook tool-before` with the payload on stdin.
3. The CLI maps the call to a `ToolRequest` and asks `ToolPolicy::evaluate`. That checks the paths, secret files,
   shell arguments (`command_risk` plus the argument check), and the verdict table.
4. Only an Allow passes. Any other verdict is blocked (decision D3). The shim throws, and OpenCode shows the reason to the model.
5. After the tool runs, `tool.execute.after` redacts secret values from the output before the model sees it.
6. Every decision is written to `.local-agent/tool-events.jsonl`, with secret values kept out.

Policy hooks fail closed. If `agent hook` cannot answer (bad config, missing binary), the shim blocks the call.

## Where state lives

- In the repo: `local-agent.toml` (project config), `opencode.json` (generated), `.opencode/plugins/local-agent.ts` (shim copy).
- In the repo's `.local-agent/` (gitignored): `tool-events.jsonl`, `events.jsonl`, `generated.json` (hashes of generated files),
  `logs/` (server logs), `servers/` (PID records).
- On the machine: `%APPDATA%\local-agent\config.toml` (model and runtime locations; written by `agent init --machine`).
- Model weights and runtimes live on `D:` only (decision D-2). Nothing large is written to `C:`.

## MCP tools

`agent mcp` serves newline-delimited JSON-RPC over stdio. Tools (registered as `local-agent_<name>`):
`git_status`, `git_diff`, `run_verification`, `harness_info`. `run_verification` runs only the steps listed in the
project config, and each step is classified by the same command policy, so a risky step is blocked, not run.

## Not yet built

Planned for later milestones: the state machine and loop detection (M3), the LFM decision engine (M4),
the Bonsai deep path (M5), context management (M6), Jev and LiteLLM (M7, M8), and the WSL runtime (release 2).
