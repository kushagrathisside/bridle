# Module: index of the module pages

Code-level reference for the three crates of the Bridle harness, one page per module group. Each page lists the
public items of its modules, the rules the code enforces, the tests that cover them, and the known gaps. These
pages describe code; they do not set policy. The system view is in [architecture.md](../../concepts/architecture.md).

> **Status:** Covers the code as of milestone M2 (tagged `m2`, 2026-10-22). Verified status lives in
> [progress.md](../../status/progress.md); where a page and that log disagree, the log wins.

## Responsibilities

| Page | Crate | Summary |
|---|---|---|
| [core-config.md](core-config.md) | `harness-core` | `identity`, `config` (schema, layering, trust, validation V1 to V25), `models` registry, `telemetry` types |
| [core-security.md](core-security.md) | `harness-core` | `security`: path guard, redaction, command classifier, tool policy |
| [runtime-providers.md](runtime-providers.md) | `harness-runtime` | `providers` (trait, OpenAI-compatible, llama.cpp, mock) and `http` |
| [runtime-serving.md](runtime-serving.md) | `harness-runtime` | `launcher`, `health`, `manifest`, `sysprobe`, `event_log` |
| [runtime-mcp.md](runtime-mcp.md) | `harness-runtime` | `mcp`: JSON-RPC protocol and the MCP tools (M1 and M2) |
| [cli-commands.md](cli-commands.md) | `harness-cli` | The `agent` commands and the `App` context |
| [cli-host-opencode.md](cli-host-opencode.md) | `harness-cli` | OpenCode adapter, hook protocol, `opencode.json` generation, the TypeScript shim |
| [orchestrator.md](orchestrator.md) | `harness-core`, `harness-runtime`, `harness-cli` | Task, journal, loops, decisions, state store, hook and `agent task` glue (M3) |
| [context.md](context.md) | `harness-core`, `harness-cli` | Context policy, the per-request transforms and the task block for the system prompt (M6) |

## Behaviour

Crates and dependency direction:

```
harness-cli  ──▶  harness-runtime  ──▶  harness-core
   (agent)          (I/O: HTTP,           (pure logic: config,
                     processes, files)     policy, redaction)
opencode-plugin/local-agent.ts  ──spawns──▶  agent hook <event>
OpenCode  ──stdio MCP──▶  agent mcp
```

- `harness-core` depends on no other workspace crate. It reads config files and canonicalises paths, and does
  nothing else that touches the system: no network, no processes.
- `harness-runtime` depends on `harness-core`. It never decides policy; it asks `harness-core`.
- `harness-cli` depends on both. Only `src/host/opencode_v1.rs` knows OpenCode payloads and config.
- The TypeScript shim `opencode-plugin/local-agent.ts` holds no policy logic.

Conventions the code follows:

1. Fail closed. A policy hook that cannot answer blocks the call. An unparseable command is classed as risky.
   Only an explicit Allow passes; Ask is treated as a denial (decision D3).
2. No panics in non-test code. Errors are values; telemetry I/O errors never stop the agent.
3. Pure functions for anything testable: argv building, parsers, classification, config merging. I/O sits at the
   edges.
4. The environment is injected (`env: &dyn Fn(&str) -> Option<String>`), so tests never mutate the process
   environment.
5. Windows rules are a parameter, not only `cfg(windows)`, so Linux test runs exercise them (path guard,
   PowerShell and `cmd` lexers).
6. Every user-visible name comes from `harness_core::identity`.

## Tests

Each page names the tests for its modules. Black-box suites live in `crates/*/tests/`; unit tests sit in the
module they cover.

## Limits

- Four modules have no page yet: `harness_core::failures` (failure parsing), `harness_runtime::code`
  (tree-sitter code intelligence, [ADR-0003](../../decisions/0003-tree-sitter-for-code-intelligence.md)),
  `harness_runtime::code::deps` (manifest parsing) and `harness_runtime::artifacts` (result-file summaries).
  [runtime-mcp.md](runtime-mcp.md) lists the tools that use them.
- Update a page in the same commit as the behaviour change it describes. Each page lists its open findings. When
  a finding is fixed, remove it from the page and record the fix in [progress.md](../../status/progress.md).

## Related

- Concepts: [architecture.md](../../concepts/architecture.md), [security.md](../../concepts/security.md)
- Decisions: [docs/decisions/](../../decisions/)
- Requirements: [requirements-traceability.md](../requirements-traceability.md)
- Status: [progress.md](../../status/progress.md)
