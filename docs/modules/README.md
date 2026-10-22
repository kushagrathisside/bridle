# Module documentation

Code-level documentation, one page per module group. For the system view, start with
[architecture.md](../architecture.md). For decisions, see [docs/adr/](../adr/). For what is verified, see
[progress.md](../progress.md).

## Crates and dependency direction

```
harness-cli  ──▶  harness-runtime  ──▶  harness-core
   (agent)          (I/O: HTTP,           (pure logic: config,
                     processes, files)     policy, redaction)
opencode-plugin/local-agent.ts  ──spawns──▶  agent hook <event>
OpenCode  ──stdio MCP──▶  agent mcp
```

- `harness-core` depends on no other workspace crate. It reads config files and canonicalises paths, and does
  nothing else that touches the system.
- `harness-runtime` depends on `harness-core`. It never decides policy; it asks `harness-core`.
- `harness-cli` depends on both. Only `src/host/opencode_v1.rs` knows OpenCode payloads and config.
- The TypeScript shim holds no policy logic.

## Pages

| Page | Covers |
|---|---|
| [core-config.md](core-config.md) | `identity`, `config` (schema, layering, validation V1 to V21), `models` registry, `telemetry` types |
| [core-security.md](core-security.md) | `security`: path guard, redaction, command classifier, tool policy |
| [runtime-providers.md](runtime-providers.md) | `providers` (trait, OpenAI-compatible, llama.cpp, mock), `http` |
| [runtime-serving.md](runtime-serving.md) | `launcher`, `health`, `manifest`, `sysprobe`, `event_log` |
| [runtime-mcp.md](runtime-mcp.md) | `mcp`: JSON-RPC protocol and the four tools |
| [cli-commands.md](cli-commands.md) | `agent` commands, `App` context |
| [cli-host-opencode.md](cli-host-opencode.md) | OpenCode adapter, hook protocol, `opencode.json` generation, the TypeScript shim |

## Conventions used in the code

- Fail closed. A policy hook that cannot answer blocks the call. An unparseable command is classed as risky.
  Only an explicit Allow passes (decision D3).
- No panics in non-test code. Errors are values; telemetry I/O errors never stop the agent.
- Pure functions for anything testable: argv building, parsers, classification, config merging. I/O sits at the edges.
- The environment is injected (`env: &dyn Fn(&str) -> Option<String>`), so tests never mutate the process environment.
- Windows rules are a parameter, not only `cfg(windows)`, so Linux CI exercises them (path guard, `cmd` lexer).
- Every user-visible name comes from `harness_core::identity`.

## Keeping these pages current

Update the page in the same commit as a behaviour change. Each page lists its open findings. When a finding
is fixed, remove it from the page and record the fix in [progress.md](../progress.md).
