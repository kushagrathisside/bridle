# Module: configuration, model registry, telemetry types (`harness-core`)

Crate: `crates/harness-core`. Pure logic over data: it reads config files, but never opens a network
socket, never spawns a process, and knows nothing about OpenCode (ADR-0001).

| File | Purpose |
|---|---|
| `src/identity.rs` | Every user-visible name: `local-agent`, binary `agent`, `.local-agent/`, `local-agent.toml`, `LOCAL_AGENT_*` |
| `src/config/schema.rs` | The `Config` type tree with serde defaults |
| `src/config/load.rs` | Layered loading and deep merge |
| `src/config/validate.rs` | Semantic rules V1 to V21 |
| `src/config/mod.rs` | Re-exports; path resolution helpers |
| `src/models/mod.rs` | `ModelRegistry`: read-only lookup of model slots by name, role or capability |
| `src/telemetry.rs` | `Event` and `EventKind`: the JSON-line record written by the event log |

## Identity (`identity.rs`)

The product name is provisional (R-0.7). Every derived string is a constant here, so a rename touches this
file, the `[[bin]]` name in `crates/harness-cli/Cargo.toml`, and the shim's `package.json`.

| Constant | Value | Used for |
|---|---|---|
| `PRODUCT_ID` | `local-agent` | MCP server name, provider prefix, machine config directory |
| `CLI_NAME` | `agent` | the binary |
| `STATE_DIR` | `.local-agent` | per-repo state (event logs, PID files, server logs) |
| `CONFIG_FILE` | `local-agent.toml` | repo config, discovered upwards from the workspace |
| `ENV_PREFIX` | `LOCAL_AGENT_` | `env_var("CONFIG")` gives `LOCAL_AGENT_CONFIG` |

OpenCode exposes MCP tools as `<server>_<tool>`, so the tools appear as `local-agent_git_status` and so on.

## Schema (`schema.rs`)

`Config` sections: `profile`, `agent`, `paths`, `host`, `runtimes`, `models`, `routing`, `decisions`, `context`,
`security`, `verification`, `telemetry`, `external`. Unknown keys are parse errors (`deny_unknown_fields`), so
a typo fails loudly instead of being ignored.

Key types:

- `ProfileKind`: `local-only`, `development`, `local-with-jev`, `hybrid`, `benchmark`, `test`.
- `RuntimeConfig { kind, server_bin, build }`: `kind` is `llama_cpp` or `prism_llama_cpp`; `build` is the
  release tag recorded in manifests (for example `b11756`).
- `ModelConfig`: `provider` (`openai_compatible`, `llama_cpp`, `prism_bonsai`, `mock`), `id`, `role`
  (`engineer`, `decision`, `deep_reasoning`, `review`, `external`), `endpoint`, `api_key_env` (the name of a
  variable, never the key), `context`, `max_output_tokens`, `sampling`, `reasoning`, `mtp`, `kv_cache`,
  `timeout_ms`, `retries`, `hardware`, `capabilities`, optional `launch` and `source`.
- `LaunchConfig`: `runtime`, `model_file`, `gpu_layers`, `threads`, `flash_attn`, `load_mode`
  (`auto`, `mmap`, `none`), `parallel` (default 1), `cache_ram_mib`, `extra_args`.
- `ModelSource { repo, revision, file, quantization, license }`: pinned download source, used by manifests.
- `SecuritySection`: verdicts for `network`, `destructive_commands`, `privileged_commands`, `risky_commands`
  (defaults Ask, Ask, Deny, Ask), `secrets_redaction` (default on), `require_worktree`, `allowed_paths`,
  `safe_commands` (extra safe prefixes; see [core-security.md](core-security.md)).
- `VerificationSection { enabled, required_for_done, default_timeout_ms (600 000), steps }`.
- `ContextSection { target_tokens (16 000), hard_limit (24 000), compact_tool_outputs }`. Profiles set
  `hard_limit` to 16 384 to match the server context (rule V21).

Path helpers in `config/mod.rs`:

- `Config::resolve_model_file(slot)` and `Config::resolve_runtime_bin(runtime)` join a relative path to
  `paths.models_dir` or `paths.runtimes_dir`. They return `None` when the path is relative and no directory is set.
- `is_absolute_any_platform(path)` treats `D:\x`, `D:/x` and `\\server\share` as absolute on every OS, so a
  Windows config validates the same way in WSL tests.

## Layered loading (`load.rs`)

Layers, lowest precedence first:

1. serde defaults;
2. machine config: `$LOCAL_AGENT_HOME/config.toml`, else `%APPDATA%\local-agent\config.toml` on Windows, else
   `$XDG_CONFIG_HOME/local-agent/config.toml` or `~/.config/local-agent/config.toml`;
3. repo config: the nearest `local-agent.toml` walking up from the workspace;
4. explicit file: `--config` or `LOCAL_AGENT_CONFIG` (must exist);
5. environment: `LOCAL_AGENT_MODELS_DIR`, `LOCAL_AGENT_RUNTIMES_DIR` (non-empty values only).

`merge_toml` deep-merges tables key by key. Any other value, arrays included, replaces the lower layer's
value. So a repo's `verification.steps` replaces the machine's list; it does not append.

`load_layered(sources, env)` takes the environment as a closure, so tests never touch the process environment.
It returns `LoadedConfig { config, files }`; `files` is shown by `agent config show`.

Errors (`ConfigError`): `NotFound` (explicit file missing), `Io`, `Parse { path, message }`.

Errors also include `Untrusted { path, key, reason }` for a repository layer that sets something only the
machine config or an explicit file may set.

**Trust (`config/trust.rs`).** The repository layer arrives with the repository, so it is checked before it is
merged. It may not set `[paths]`, `host.opencode_bin`, a `server_bin` or `model_file` outside the configured
directories (absolute, `..`, a drive, `~`, `$`, `%`), a non-loopback endpoint, `api_key_env`, a remote profile,
external providers (other than `enabled = false`), `safe_commands` or `allowed_paths`, or anything that turns a
protection off or makes a verdict looser than the layers below. The machine config and `--config` are trusted.
The shipped profiles pass the check (tested). User guide: [configuration.md](../configuration.md).

## Validation (`validate.rs`)

`validate_with(cfg, env)` returns every issue at once (`ConfigIssue { severity, path, message }`), with dotted
paths such as `models.main.endpoint`. `ValidationEnv` carries the system drive, so the D-2 check is testable.

| Rule | Severity | Checks |
|---|---|---|
| V1, V2 | Error | `models` is empty; `agent.default_model` names a missing slot |
| V3, V4 | Error | endpoint is not `http(s)://host`; host is not loopback unless cloud models are enabled over https; a non-local model without cloud models enabled |
| V5 | Error | `context == 0` or `max_output_tokens >= context` |
| V6, V7 | Error | launch runtime missing; ternary (Bonsai) weights on a stock llama.cpp runtime, or `prism_bonsai` on a non-prism runtime |
| V8 | Error | `api_key_env` is not an environment variable name (it probably holds the key itself) |
| V9, V21 | Error | `target_tokens > hard_limit`; `hard_limit` above the default model's context |
| V10, V13 | Error | escalation threshold or sampling values out of range |
| V11, V12 | Error | Jev used while disabled; any external provider enabled under `local-only` |
| V14 | Warning | unknown `kv_cache` type |
| V15 | Warning | verification required but no steps configured |
| V16 | Error | relative `model_file`/`server_bin` with no base directory |
| V17 | Warning | two slots share one endpoint |
| V18 | Warning | `models_dir` on the system drive (directive D-2) |
| V19 | Warning | decision fallback is not `rules` |
| V20 | Error | `mock` provider outside the `test` profile |
| V22 | Error | `launch.extra_args` contains a reserved flag (`RESERVED_LAUNCH_FLAGS`: `--host`, `--port`, `-m`, `--alias`, `-c`, `--api-key`, `--log-file`, `--slot-save-path`, model download flags, ...) |
| V23 | Error | `timeout_ms` outside 1 s..1 h, or `retries` above 10 |

`reserved_launch_flag(extra_args)` in `config/mod.rs` is shared with the launcher, which refuses the same flags.

## Model registry (`models/mod.rs`)

`ModelRegistry::from_config(cfg)` is a sorted, read-only view of the slots. `get(slot)`, `slots()`,
`find(&CapabilityRequirement)` (each `Some(true)` field is required, `None` is "don't care", plus a minimum
context and a local-only flag) and `first_with_role(role)`. Order is deterministic, so routing is reproducible.

## Telemetry types (`telemetry.rs`)

`Event { timestamp, kind, ... }` with optional fields omitted from JSON. `EventKind`: `tool_before`, `tool_after`,
`permission`, `shell_env`, `session`, `model_call`, `health_check`, `decision`, `verification`, `mcp`, `doctor`.
`Event::now(kind).with(key, value)` builds one record. Writing and redaction happen in
`harness-runtime::event_log` ([runtime-serving.md](runtime-serving.md)).

## Tests

- `config/load.rs`: merge semantics, discovery, precedence, environment overrides, unknown keys, missing explicit
  file, and a repository layer refused where an explicit file with the same content is accepted.
- `config/trust.rs`: shipped profiles accepted; every refusal; stricter security accepted; comparison against
  the machine layer.
- `config/validate.rs`: a clean baseline plus at least one failing case per rule.
- `config/mod.rs`: Windows absolute paths on Linux; path resolution.
- Profiles in `configs/*.toml` are parsed and validated by tests, so a broken shipped profile fails the build.
