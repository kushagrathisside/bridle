# ADR-0001: Rust core with a thin OpenCode host adapter

- **Status:** Accepted
- **Date:** 2026-10-21
- **Deciders:** Project lead
- **Supersedes:** The TypeScript tooling of the earlier design (ESLint, Prettier and Vitest). It also narrows the earlier
  TypeScript and tooling expectations.

## Context

The earlier design preferred TypeScript for the OpenCode extension layer, and it warned against a second language
runtime "unless it provides clear value". On 2026-10-21 the project lead directed that **Rust is the
primary implementation language**.

OpenCode itself cannot be changed. Its plugins are JavaScript or TypeScript modules that run inside
OpenCode's Bun runtime (`@opencode-ai/plugin` 1.19.3). Any hook into OpenCode therefore needs some JS or TS.
The open question was how much.

## Decision

1. **All project logic is Rust.** This covers config, model registry and routing, decision engines,
   policy, path guard, redaction, fingerprints, state machine, context manager, providers, health checks,
   telemetry, tools, eval runner and CLI. The code is a Cargo workspace of three crates:

   | Crate | Kind | Responsibility | May depend on |
   |---|---|---|---|
   | `harness-core` | lib | Pure, deterministic domain logic. No network, no process spawning, no OpenCode knowledge. | serde, regex, sha2 |
   | `harness-runtime` | lib | I/O: model providers (HTTP), health checks, manifests, process launcher, event log, MCP server, tools | `harness-core`, HTTP client |
   | `harness-cli` | bin `agent` | Entry points (`doctor`, `health`, `init`, `models`, `mcp`, `hook`, `explain`, `trace`). Owns the **only** OpenCode-specific Rust module, `host/opencode_v1.rs`. | both libs, clap |

   The crate names are neutral on purpose. The temporary product identifier (`local-agent`, binary `agent`,
   state directory `.local-agent/`, environment prefix `LOCAL_AGENT_`) lives in `harness-core::identity` and
   in the `[[bin]]` and plugin package names. A rename then touches only those places.

2. **Tools are exposed to OpenCode over MCP (stdio), not as TypeScript custom tools.** `agent mcp` is a
   small, hand-written MCP server (JSON-RPC 2.0, newline-delimited). OpenCode loads it through its stable
   `mcp` config key (`type: "local"`, `command: ["agent", "mcp"]`). This does not depend on the host: any
   MCP-capable harness can use the same tools if OpenCode changes (the design is meant to survive OpenCode changes).

3. **The OpenCode plugin is a thin TypeScript shim, with a target of under 200 lines and no logic.** It
   forwards the `tool.execute.before`, `tool.execute.after`, `permission.ask`, `shell.env` and `event`
   payloads as JSON to `agent hook <event>` (stdin to stdout). It then applies the returned verdict: it
   throws to block, sets the permission status, or changes the environment or output. It has no runtime npm
   dependencies. The `@opencode-ai/plugin` import is type-only, and Bun erases it.

4. **Configuration is TOML** (`local-agent.toml`, with profiles in `configs/*.toml`). The earlier design treats the
   format as negotiable. TOML is native to Rust, and the de facto YAML crate (`serde_yaml`) is archived
   and unmaintained.

5. **Toolchain.** Rust 1.95.0 is pinned in `rust-toolchain.toml`. This version is installed on both the
   Windows host (MSVC) and WSL, so no toolchain download is needed. The quality gates are `rustfmt`,
   `clippy -D warnings`, `cargo test`, and later `cargo-deny` for the licence and advisory audit. CI
   type-checks the TS shim with `tsc --strict --noEmit`.

### Evidence

- The `Hooks` interface of the OpenCode 1.19.3 plugin was inspected from the published
  `@opencode-ai/plugin@1.19.3` type definitions on 2026-10-21. It exposes `tool.execute.before/after`,
  `permission.ask`, `shell.env`, `chat.params`, `event`, `experimental.chat.{system,messages}.transform` and
  `experimental.session.compacting`. Each one accepts plain data that serialises to JSON, so a subprocess
  bridge loses nothing.
- OpenCode's MCP config (`McpLocalConfig`: `type`, `command[]`, `environment`, `enabled`, `timeout`) is part
  of the stable config schema. MCP tool names are prefixed `<server>_`, so OpenCode's existing permission
  globs (`"local-agent_*"`) can govern them.
- Both the Windows host (VS 2022 Build Tools 14.44, Windows SDK 10.0.26100, rustup 1.95.0-msvc) and WSL
  (1.95.0 and 1.99.0 stable) can build the workspace without new installs.

## Consequences

The source records the consequences as risks, each with its mitigation.

| Risk | Mitigation |
|---|---|
| A process spawn per hook call adds latency. Rust start-up takes roughly 5 to 15 ms on Windows, against tool calls that take seconds. | Measure it and record the result in telemetry. If the latency matters, switch the shim to a long-lived `agent serve` over a local named pipe, behind the same JSON contract. |
| The hook bridge fails, for example because the binary is missing. | The shim **fails closed** for `tool.execute.before` and `permission.ask` (block or ask). It fails open only for hooks that carry telemetry alone. |
| The repository holds two languages. | TS is confined to `opencode-plugin/` (the shim) and holds no business logic. It is reviewed as an adapter, not as a component. |
| A hand-written MCP server drifts from the specification. | Implement only `initialize`, `notifications/initialized`, `ping`, `tools/list` and `tools/call`. Negotiate the protocol version. Add a protocol conformance test. The `rmcp` crate remains an option if a richer MCP surface is needed. |
| Contributors may expect a TypeScript plugin package, as the earlier design described. | Document how the earlier design's module list maps to the Rust modules in [architecture](../concepts/architecture.md). |

## Alternatives considered

- **An all-TypeScript plugin (the earlier design's default).** Rejected by the project lead.
- **Rust compiled to WASM and loaded inside the plugin.** Process spawning, file system access and HTTP from
  WASM inside Bun would need host shims for everything. That is more complex and slower to build. Rejected.
- **A Rust N-API addon loaded by Bun.** This ties the build to Bun's Node-API compatibility and to native
  binaries for each platform inside the plugin. Rejected for the first release. It could be revisited if hook
  latency becomes a measured problem.

## Related

- [ADR-0002](0002-runtime-os-repo-location-and-deviation.md): runtime OS support and repository location. It
  amends the environment assumptions in this ADR.
- [ADR-0003](0003-tree-sitter-for-code-intelligence.md): tree-sitter for the code tools served over the MCP
  server described here.
- [Architecture](../concepts/architecture.md).
