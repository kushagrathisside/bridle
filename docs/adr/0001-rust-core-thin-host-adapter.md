# ADR-0001: Rust core with a thin OpenCode host adapter

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-10-21 |
| **Decision owner** | Project lead (directive D-1, [traceability](../spec/requirements-traceability.md#pm-directives-received-after-the-brief)) |
| **Supersedes** | R-50.2 (TypeScript/ESLint/Prettier/Vitest tooling). Narrows R-31.1 and R-50.3. |

## Context

The brief (§50) prefers TypeScript for the OpenCode extension layer and warns against a second language
runtime "unless it provides clear value". On 2026-10-21 the project lead directed that **Rust is the
primary implementation language**.

OpenCode itself cannot be changed: plugins are JavaScript/TypeScript modules that run inside OpenCode's
Bun runtime (`@opencode-ai/plugin` 1.19.3). Any hook into OpenCode therefore needs *some* JS/TS. The
question is how much.

## Decision

1. **All project logic is Rust.** This covers config, model registry and routing, decision engines,
   policy, path guard, redaction, fingerprints, state machine, context manager, providers, health checks,
   telemetry, tools, eval runner and CLI. It is a Cargo workspace of three crates:

   | Crate | Kind | Responsibility | May depend on |
   |---|---|---|---|
   | `harness-core` | lib | Pure, deterministic domain logic. No network, no process spawning, no OpenCode knowledge. | serde, regex, sha2 |
   | `harness-runtime` | lib | I/O: model providers (HTTP), health checks, manifests, process launcher, event log, MCP server, tools | `harness-core`, HTTP client |
   | `harness-cli` | bin `agent` | Entry points (`doctor`, `health`, `init`, `models`, `mcp`, `hook`, `explain`, `trace`). Owns the **only** OpenCode-specific Rust module, `host/opencode_v1.rs`. | both libs, clap |

   Crate names are neutral on purpose. The temporary product identifier (`local-agent`, binary `agent`,
   state dir `.local-agent/`, env prefix `LOCAL_AGENT_`) lives in `harness-core::identity` and in the
   `[[bin]]` / plugin package names. That way a rename touches only those places (R-0.7).

2. **Tools are exposed to OpenCode over MCP (stdio), not as TypeScript custom tools.** `agent mcp`
   is a small, hand-written MCP server (JSON-RPC 2.0, newline-delimited). OpenCode loads it through its
   stable `mcp` config key (`type: "local"`, `command: ["agent", "mcp"]`). This is host-agnostic: any
   MCP-capable harness can use the same tools if OpenCode changes (§66 "design survives OpenCode changes").

3. **The OpenCode plugin is a thin TypeScript shim (target: under 200 lines, no logic).** It forwards
   `tool.execute.before`, `tool.execute.after`, `permission.ask`, `shell.env` and `event` payloads as
   JSON to `agent hook <event>` (stdin → stdout). It then applies the returned verdict: throw to block,
   set permission status, or mutate env/output. It has no runtime npm dependencies. The
   `@opencode-ai/plugin` import is type-only and is erased by Bun.

4. **Configuration is TOML** (`local-agent.toml`, profiles in `configs/*.toml`). The brief says the format
   is negotiable (§7). TOML is Rust-native, and the de-facto YAML crate (`serde_yaml`) is archived and
   unmaintained.

5. **Toolchain:** Rust 1.95.0 pinned via `rust-toolchain.toml`. This is the version installed on both
   the Windows host (MSVC) and WSL, so no toolchain download is needed. Quality gates: `rustfmt`,
   `clippy -D warnings`, `cargo test`, and later `cargo-deny` for licence and advisory audit (M11). The
   TS shim is type-checked with `tsc --strict --noEmit` in CI.

## Evidence

- OpenCode 1.19.3 plugin `Hooks` interface (inspected from the published `@opencode-ai/plugin@1.19.3`
  type definitions on 2026-10-21) exposes `tool.execute.before/after`, `permission.ask`, `shell.env`,
  `chat.params`, `event`, `experimental.chat.{system,messages}.transform` and
  `experimental.session.compacting`. Every one of them accepts plain JSON-serialisable data, so a
  subprocess bridge loses nothing.
- OpenCode's MCP config (`McpLocalConfig`: `type`, `command[]`, `environment`, `enabled`, `timeout`) is
  part of the stable config schema. MCP tool names are prefixed `<server>_`, so they can be governed with
  OpenCode's existing permission globs (`"local-agent_*"`).
- Both the Windows host (VS 2022 Build Tools 14.44, Windows SDK 10.0.26100, rustup 1.95.0-msvc) and WSL
  (1.95.0 and 1.99.0 stable) can build the workspace without new installs.

## Consequences and risks

| Risk | Mitigation |
|---|---|
| A process spawn per hook call adds latency. Rust start-up is roughly 5–15 ms on Windows, against tool calls that take seconds. | Measure in M1 and record in telemetry. If it matters, switch the shim to a long-lived `agent serve` over a local named pipe behind the same JSON contract. |
| The hook bridge fails, e.g. the binary is missing. | The shim **fails closed** for `tool.execute.before` and `permission.ask` (block or ask), and fails open only for telemetry-only hooks. |
| Two languages in the repo. | TS is confined to `opencode-plugin/` (the shim) and holds no business logic. It is reviewed as an adapter, not a component. |
| A hand-written MCP server diverges from the spec. | Implement only `initialize`, `notifications/initialized`, `ping`, `tools/list` and `tools/call`. Negotiate protocol version. Add a protocol conformance test. The `rmcp` crate remains an option if a richer MCP surface is needed. |
| Contributors expect a TS plugin package (§31, §51). | Document the mapping from §31's module list to the Rust modules in `docs/architecture.md`. |

## Alternatives considered

- **All-TypeScript plugin (the brief's default).** Rejected by PM directive D-1.
- **Rust compiled to WASM and loaded inside the plugin.** Process spawning, file system and HTTP from WASM
  inside Bun would need host shims for everything, which is more complex and slower to build. Rejected.
- **Rust N-API addon loaded by Bun.** Ties the build to Bun's Node-API compatibility and to
  per-platform native binaries inside the plugin. Rejected for M1. It could be revisited if hook latency
  becomes a measured problem.
