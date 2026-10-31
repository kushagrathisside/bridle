# Module: harness-runtime/serving

The modules that start and check local model servers and record what ran: the launcher, functional health checks,
model manifests, host probes, and the append-only event log. They own process spawning for `llama-server`, the
hash cache, and the JSONL files under `.local-agent/`. They make no policy decisions; reserved launch flags and
redaction rules come from `harness-core`.

> **Status:** Built and tested. Verified on Windows with Qwen3.5-9B and llama.cpp b11756.

## Responsibilities

| File | Public items | What they do | Used by |
|---|---|---|---|
| `src/launcher.rs` | `server_args`, `parse_endpoint`, `spawn_server`, `wait_until_healthy`, `LaunchError` | Builds the `llama-server` argv, spawns the server, waits for health | `agent models serve` |
| `src/health.rs` | `run_checks`, `summarize`, `CheckKind`, `CheckResult`, `CheckOptions`, `Status` | Functional health checks (8 checks) | `agent health` |
| `src/manifest.rs` | `sha256_file`, `cached_sha256`, `hash_cache_path`, `parse_llama_version`, `build_manifest`, `write_manifest`, `ModelManifest` | Model manifests: hash, revision, runtime build, hardware | `agent models manifest`, `agent doctor` |
| `src/sysprobe.rs` | `probe_system`, `probe_nvidia`, `probe_wsl`, `free_space`, `command_version`, `command_output`, `process_image`, parsers | OS, CPU, RAM, GPU, CUDA, WSL, disk, tool versions | `agent doctor`, manifests, `agent models stop` |
| `src/event_log.rs` | `EventLog` (`for_workspace`, `append`, `dir`), `TOOL_EVENTS`, `EVENTS`, `truncate_strings` | Append-only JSONL log, redacted on write | hooks, MCP, commands |

## Behaviour

### Launcher (`launcher.rs`)

`server_args(model, launch, model_file, allow_remote_bind)` is pure and unit-tested. It emits, in order:

```
-m <model_file> --host <host> --port <port> --alias <model.id> -c <context> --jinja --metrics
[-ngl <gpu_layers>] [-t <threads>] [-fa on|off] [-ctk <kv> -ctv <kv>] [--load-mode <mode>]
-np <parallel> [--cache-ram <MiB>] [--reasoning-budget <n>] <extra_args...>
```

1. Host and port come from `model.endpoint` (`parse_endpoint`). A missing port is `LaunchError::Endpoint`.
2. A non-loopback host is `LaunchError::NonLoopbackBind` unless the caller passes `allow_remote_bind`, so a server
   never listens on the LAN by accident.
3. `extra_args` may not contain a reserved flag (`--host`, `--port`, `-m`, `--log-file`, ...): validation rule V22
   rejects it, and `server_args` refuses it too (`LaunchError::ReservedFlag`), so a skipped validation cannot
   re-bind the server.
4. Flags match llama.cpp b11756: there is no `--no-mmap` in that build (`--load-mode` replaces it), and no
   `--spec-type` unless the user adds it. The profiles set `load_mode = "mmap"`, `parallel = 1` and
   `cache_ram_mib = 1024`; the server's default prompt cache (8 GiB) would starve a 16 GB host.
5. Model file and alias are separate argv elements. No shell is involved.

`spawn_server(bin, args, log_file)` appends stdout and stderr to the log file and sets `CREATE_NO_WINDOW` on
Windows. A missing binary is `LaunchError::MissingBinary(path)`.

`wait_until_healthy(health_url, timeout, poll, child)` polls `/health` (2 s per request,
`HEALTH_REQUEST_TIMEOUT`) until HTTP 200 and returns the elapsed load time. It fails fast with
`LaunchError::Exited` if the server dies, and with `NotReady` on timeout.

### Health checks (`health.rs`)

An HTTP 200 does not prove the model works. `run_checks(provider, model, opts)` runs, in order:

| # | Check | Pass when |
|---|---|---|
| 1 | Health | `/health` is ok. If it fails, every later check is Skip ("endpoint not healthy") |
| 2 | Models | the server lists `model.id` (Warn if reachable but the id differs) |
| 3 | Completion | `Reply with the single word READY.` returns `READY` (thinking off, 32 tokens, temperature 0) |
| 4 | Streaming | at least 2 deltas; records time to first token (Skip without the capability) |
| 5 | ToolCall | a `get_weather` call whose `city` contains Paris (Warn if malformed; Skip without the capability) |
| 6 | StructuredOutput | JSON matching a strict schema (`decision` enum, `confidence` in 0..1; Skip without the capability) |
| 7 | Context | server `n_ctx >= model.context` (Warn if smaller; Skip without props) |
| 8 | Mtp | informational: speculative decoding detected or not (Skip when `mtp` is off) |

`opts.quick` runs checks 1 to 3. `/props` is fetched once for checks 7 and 8. The Mtp check treats the strings
`"none"`, `"off"`, `"false"`, `"disabled"`, `"0"` and `""` as off; llama.cpp b11756 reports `"types": "none"`
when no speculative decoding is configured. `summarize` gives Fail if any check failed, else Warn if any warned,
else Pass; Skip never counts. The checks never panic. Verified on Windows with Qwen3.5-9B: all checks pass in
2.5 s, 98 ms to first token.

### Manifests (`manifest.rs`)

A manifest records what was actually run, so benchmark results are comparable.

- `sha256_file(path)`: streaming SHA-256 with a 1 MiB buffer (`HASH_BUFFER_BYTES`).
- `cached_sha256(cache_path, file)`: cache at `<state>/manifests/hash-cache.json` (`hash_cache_path`), keyed by
  absolute path, valid while size and mtime match. Hashing a 6 GB file takes tens of seconds; the cache makes
  repeat runs fast.
- `parse_llama_version(text)`: reads `version: 11756 (a1b2c3d)` from `--version` output (stdout or stderr; `\r\n`
  tolerated).
- `build_manifest(input)`: pure. `verified` is `None` when no expected hash is configured, otherwise whether the
  hash matches `source.sha256` (case-insensitive).
- `write_manifest(dir, manifest)`: `<dir>/<slot>.json`, pretty JSON. Slot names that are paths are refused.

### Host probes (`sysprobe.rs`)

Every probe is best effort: a failure becomes `None` or an empty list, never an error or a panic. External commands
run with a 5 s timeout (`PROBE_TIMEOUT`).

- `probe_system()`: OS, kernel, CPU brand and count, total and available memory (`sysinfo`), GPUs, WSL facts.
- `probe_nvidia()`: `nvidia-smi --query-gpu=...` parsed by `parse_nvidia_csv`; CUDA version from plain
  `nvidia-smi` (`parse_cuda_version`).
- `probe_wsl()`: inside WSL (from `/proc/sys/kernel/osrelease`); on Windows, `.wslconfig` limits
  (`parse_wslconfig`) and running distributions (`wsl.exe -l --running -q`, UTF-16LE decoded by
  `decode_utf16le_lossy`).
- `free_space(path)`: free bytes on the volume holding `path` (longest mount-point prefix).
- `command_version(bin, args)`: first non-empty line of a tool's version output. `command_output` returns all of
  it (used for `llama-server --version`). Both run with the 5 s timeout.
- `process_image(pid)`: the executable name of a running process (`tasklist` on Windows, parsed by
  `parse_tasklist_csv`; `/proc` or `ps` elsewhere), used by `agent models stop` before it signals anything.

### Event log (`event_log.rs`)

`EventLog::for_workspace(workspace, redact, max_field_bytes, enabled)` writes to `<workspace>/.local-agent/`:
`tool-events.jsonl` (`TOOL_EVENTS`: `tool_before`, `tool_after` and `permission` events) and `events.jsonl`
(`EVENTS`: everything else).

1. A disabled log writes nothing.
2. `append(event)` redacts `data` (by key and by pattern) and the top-level text fields copied from host payloads
   (`tool`, `status`, `actor`, `model`, `phase`; by pattern). Task, session and call ids are kept for correlation.
3. It caps every string at `telemetry.max_field_bytes` (char-safe, `truncate_strings`).
4. It writes one line with a single `write_all` in append mode, which keeps lines whole when several hook processes
   append at once.

Telemetry must not break the agent: I/O errors are returned for the caller to ignore or report, never panicked on.

## Tests

- `tests/launcher_wait.rs`: `launcher_wait_succeeds_after_two_503_answers`,
  `launcher_wait_times_out_when_nothing_listens`, `launcher_wait_fails_fast_when_the_child_exits`,
  `launcher_args_snapshot_for_a_gpu_model`, `launcher_args_snapshot_for_a_cpu_model`,
  `launcher_args_put_extra_args_last_verbatim`.
- `src/launcher.rs`: optional flags, endpoint parsing, loopback rules, IPv6, missing binary, reserved flags.
- `tests/health_mock.rs` (14 tests, with `MockProvider::scripted`): each pass, warn and fail path, including
  `unhealthy_endpoint_fails_health_and_skips_the_rest`, `quick_mode_runs_only_three_checks`,
  `context_check_compares_server_n_ctx_with_config`, `mtp_check_detects_speculative_or_draft_keys` and
  `summarize_fails_over_warns_and_ignores_skips`.
- `src/manifest.rs`: known digest, cache hit and miss, verified flag, version parsing,
  `write_manifest_rejects_slot_names_that_are_paths`.
- `src/sysprobe.rs`: every pure parser, using real `nvidia-smi` output from the target laptop.
- `src/event_log.rs`: truncation at char boundaries; an event with secrets in `data` and in `tool` is written and
  the file is read back with no secret in it; a disabled log writes nothing.

## Limits

- With `secrets_redaction = false`, event fields are written raw (config-gated; flagged by the redaction review).
- Deferred to a later release: starting the server in its own process group on Unix. That would stop Ctrl+C from
  reaching an attached `serve`; on Windows the server has no console.
- The launcher argv matches llama.cpp b11756; another build may need different flags.

## Related

- Modules: [runtime-providers.md](runtime-providers.md), [core-config.md](core-config.md),
  [cli-commands.md](cli-commands.md), [index](README.md)
- Concepts: [architecture.md](../../concepts/architecture.md), [security.md](../../concepts/security.md)
- Decisions: [docs/decisions/](../../decisions/)
- Guide: [models.md](../../guide/models.md), [troubleshooting.md](../../guide/troubleshooting.md)
