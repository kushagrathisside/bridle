# Module: harness-core/config

Configuration for the harness: the product identity constants, the `Config` type tree, layered loading with a
trust check on the repository layer, semantic validation, the read-only model registry, and the telemetry event
types. It reads config files and nothing else: it never opens a network socket, never spawns a process, and knows
nothing about OpenCode (ADR-0001).

> **Status:** Built and tested. Identity, schema, loading, validation V1 to V35, registry and telemetry types are
> from M1; the `[tools]` section, rules V24 and V25, and the repository-layer refusal of `tools` were added in M2.

## Responsibilities

| File | Public items | What they do |
|---|---|---|
| `src/identity.rs` | `PRODUCT_ID`, `CLI_NAME`, `STATE_DIR`, `CONFIG_FILE`, `ENV_PREFIX`, `MCP_SERVER_NAME`, `PROVIDER_PREFIX`, `MACHINE_CONFIG_DIR`, `MACHINE_CONFIG_FILE`, `env_var` | Every user-visible name |
| `src/config/schema.rs` | `Config` and its section types, enums with serde defaults | The config type tree |
| `src/config/load.rs` | `load_layered`, `merge_toml`, `discover_repo_config`, `machine_config_path`, `from_toml_str`, `ConfigSources`, `LoadedConfig`, `ConfigError` | Layered loading and deep merge |
| `src/config/trust.rs` | `check_repo_layer` (crate-private) | What a repository config may set |
| `src/config/validate.rs` | `validate`, `validate_with`, `ValidationEnv`, `ConfigIssue`, `Severity` | Semantic rules V1 to V25 |
| `src/config/mod.rs` | `Config::resolve_model_file`, `Config::resolve_runtime_bin`, `is_absolute_any_platform`, `RESERVED_LAUNCH_FLAGS`, `reserved_launch_flag` | Re-exports and path helpers |
| `src/models/mod.rs` | `ModelRegistry`, `CapabilityRequirement` | Read-only lookup of model slots by name, role or capability |
| `src/telemetry.rs` | `Event`, `EventKind` | The JSON-line record written by the event log |

## Behaviour

### Identity (`identity.rs`)

The product name is provisional (R-0.7). Every derived string is a constant here, so a rename touches this file,
the `[[bin]]` name in `crates/harness-cli/Cargo.toml`, and the shim's `package.json`.

| Constant | Value | Used for |
|---|---|---|
| `PRODUCT_ID` | `local-agent` | MCP server name (`MCP_SERVER_NAME`), provider prefix (`PROVIDER_PREFIX`), machine config directory (`MACHINE_CONFIG_DIR`) |
| `CLI_NAME` | `agent` | The binary |
| `STATE_DIR` | `.local-agent` | Per-repository state (event logs, PID files, server logs) |
| `CONFIG_FILE` | `local-agent.toml` | Repository config, discovered upwards from the workspace |
| `ENV_PREFIX` | `LOCAL_AGENT_` | `env_var("CONFIG")` gives `LOCAL_AGENT_CONFIG` |
| `MACHINE_CONFIG_FILE` | `config.toml` | File name inside the machine config directory |

OpenCode exposes MCP tools as `<server>_<tool>`, so the tools appear as `local-agent_git_status` and so on.

### Schema (`schema.rs`)

`Config` sections: `profile`, `agent`, `paths`, `host`, `runtimes`, `models`, `routing`, `decisions`, `context`,
`security`, `verification`, `telemetry`, `external`, `tools`. Unknown keys are parse errors (`deny_unknown_fields`),
so a typo fails instead of being ignored.

- `ProfileKind`: `local-only`, `development`, `local-with-jev`, `hybrid`, `benchmark`, `test`.
- `RuntimeConfig { kind, server_bin, build }`: `kind` is `llama_cpp` or `prism_llama_cpp`; `build` is the
  release tag recorded in manifests (for example `b11756`).
- `ModelConfig`: `provider` (`openai_compatible`, `llama_cpp`, `prism_bonsai`, `mock`), `id`, `display_name`,
  `role` (`engineer`, `decision`, `deep_reasoning`, `review`, `external`), `endpoint`, `api_key_env` (the name of
  a variable, never the key), `context`, `max_output_tokens` (default 4096), `sampling`, `reasoning`, `mtp`,
  `kv_cache`, `timeout_ms` (default 300 000), `retries` (default 1), `hardware`, `capabilities`, optional `launch`
  and `source`.
- `LaunchConfig`: `runtime`, `model_file`, `gpu_layers`, `threads`, `flash_attn`, `load_mode` (`auto`, `mmap`,
  `none`), `parallel` (default 1), `cache_ram_mib`, `extra_args`.
- `ModelSource { repo, revision, file, sha256, quantization, license }`: pinned download source, used by manifests.
- `SecuritySection`: verdicts for `network`, `destructive_commands`, `privileged_commands`, `risky_commands`
  (defaults Ask, Ask, Deny, Ask), `secrets_redaction` (default on), `require_worktree` (default on),
  `allowed_paths`, `safe_commands` (extra safe prefixes; see [core-security.md](core-security.md)).
- `VerificationSection { enabled, required_for_done, default_timeout_ms (600 000), steps }`. Each
  `VerificationStep` has `name`, `command`, `timeout_ms`, `cwd`, `kind` (`StepKind`: `test` by default, `lint`,
  `typecheck`, `build`, `other`) and `targets`.
- `ContextSection { target_tokens (16 000), hard_limit (24 000), compact_tool_outputs }`. Profiles set
  `hard_limit` to 16 384 to match the server context (rule V21).
- `TelemetrySection { enabled, destination (local only), max_field_bytes (2048) }`.
- `ToolsSection { adapters }`: `[[tools.adapters]]` entries (`AdapterConfig { name, description, program, args,
  accepts_args, timeout_ms }`), each exposed as an MCP tool named `adapter_<name>` ([runtime-mcp.md](runtime-mcp.md)).

### Path helpers (`config/mod.rs`)

- `Config::resolve_model_file(slot)` and `Config::resolve_runtime_bin(runtime)` join a relative path to
  `paths.models_dir` or `paths.runtimes_dir`. They return `None` when the path is relative and no directory is set.
- `is_absolute_any_platform(path)` treats `D:\x`, `D:/x` and `\\server\share` as absolute on every OS, so a
  Windows config validates the same way in WSL tests.
- `reserved_launch_flag(extra_args)` returns the first entry of `extra_args` that is in `RESERVED_LAUNCH_FLAGS`, as
  `--flag` or `--flag=value`. It is shared with the launcher, which refuses the same flags.

### Layered loading (`load.rs`)

Layers, lowest precedence first:

1. serde defaults;
2. machine config: `$LOCAL_AGENT_HOME/config.toml`, else `%APPDATA%\local-agent\config.toml` on Windows, else
   `$XDG_CONFIG_HOME/local-agent/config.toml` or `~/.config/local-agent/config.toml`;
3. repository config: the nearest `local-agent.toml` walking up from the workspace;
4. explicit file: `--config` or `LOCAL_AGENT_CONFIG` (must exist);
5. environment: `LOCAL_AGENT_MODELS_DIR`, `LOCAL_AGENT_RUNTIMES_DIR` (non-empty values only).

`merge_toml` deep-merges tables key by key. Any other value, arrays included, replaces the lower layer's value. So
a repository's `verification.steps` replaces the machine's list; it does not append.

`load_layered(sources, env)` takes the environment as a closure, so tests never touch the process environment. It
returns `LoadedConfig { config, files }`; `files` is shown by `agent config show`.

`ConfigError` variants: `NotFound` (explicit file missing), `Io`, `Parse { path, message }`, and
`Untrusted { path, key, reason }` for a repository layer that sets something only the machine config or an
explicit file may set.

### Trust (`config/trust.rs`)

The repository layer arrives with the repository, so it is checked before it is merged. It may not set:

- `[tools]` (a repository may not add tools);
- `[paths]` or `host.opencode_bin`;
- a `server_bin` or `model_file` outside the configured directories (absolute, `..`, a drive, `~`, `$`, `%`);
- a non-loopback endpoint, or `api_key_env`;
- a remote profile (`hybrid`, `local-with-jev`);
- anything under `external` other than `enabled = false`;
- `security.safe_commands` or `security.allowed_paths`;
- anything that turns a protection off (`secrets_redaction`, `require_worktree`) or makes a verdict looser than the
  layers below.

The machine config and `--config` are trusted. The shipped profiles pass the check (tested). User guide:
[configuration.md](../../guide/configuration.md).

### Validation (`validate.rs`)

`validate_with(cfg, env)` returns every issue at once (`ConfigIssue { severity, path, message }`, plus the rule
id), with dotted paths such as `models.main.endpoint`. `ValidationEnv` carries the system drive, so the D-2 check
is testable. `validate(cfg)` uses `ValidationEnv::from_process()`.

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
| V24 | Warning | a verification step sets `targets = true` but its `kind` is not `test` |
| V25 | Error | an adapter has a bad or duplicate name, a non-bare program, non-plain arguments, or no description |

### Model registry (`models/mod.rs`)

`ModelRegistry::from_config(cfg)` is a sorted, read-only view of the slots, with `get(slot)`, `slots()`,
`find(&CapabilityRequirement)` and `first_with_role(role)`. In a `CapabilityRequirement`, each `Some(true)` field
is required and `None` means "don't care", plus a minimum context and a local-only flag. Order is deterministic, so
routing is reproducible.

### Telemetry types (`telemetry.rs`)

`Event { timestamp, kind, ... }` with optional fields omitted from JSON. `EventKind`: `tool_before`, `tool_after`,
`permission`, `shell_env`, `session`, `model_call`, `health_check`, `decision`, `verification`, `mcp`, `doctor`.
`Event::now(kind).with(key, value)` builds one record. Writing and redaction happen in
`harness-runtime::event_log` ([runtime-serving.md](runtime-serving.md)).

## Tests

- `config/load.rs`: merge semantics, discovery, precedence, environment overrides, unknown keys, missing explicit
  file, and `repository_layer_may_not_choose_executables_but_an_explicit_file_may`.
- `config/trust.rs`: `shipped_profiles_are_valid_repository_configs`, `executables_and_locations_are_machine_settings`,
  `credentials_remote_endpoints_and_external_providers_are_rejected`, `security_may_only_get_stricter`.
- `config/validate.rs`: a clean baseline plus at least one failing case per rule, and
  `shipped_profiles_validate_without_errors`, so a broken profile in `configs/*.toml` fails the build.
- `config/mod.rs`: Windows absolute paths on Linux (`windows_drive_paths_are_absolute_everywhere`); path resolution.
- `identity.rs`: `env_var_is_prefixed`, `mcp_tool_prefix_matches_opencode_convention`.

## Limits

- The product name is provisional (R-0.7); a rename needs the three edits listed under Identity.
- Arrays never merge across layers. A repository that sets `verification.steps` replaces the machine list.
- The trust check reports the first refused key only, not every one.

## Related

- Modules: [core-security.md](core-security.md), [runtime-serving.md](runtime-serving.md),
  [runtime-mcp.md](runtime-mcp.md), [cli-commands.md](cli-commands.md), [index](README.md)
- Concepts: [architecture.md](../../concepts/architecture.md), [security.md](../../concepts/security.md)
- Decisions: [docs/decisions/](../../decisions/), in particular
  [ADR-0001](../../decisions/0001-rust-core-thin-host-adapter.md)
- Guide: [configuration.md](../../guide/configuration.md)
