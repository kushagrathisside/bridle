# Module: launcher, health checks, manifests, host probes, event log (`harness-runtime`)

| File | Purpose | Used by |
|---|---|---|
| `src/launcher.rs` | Builds the `llama-server` argv, spawns the server, waits for health | `agent models serve` |
| `src/health.rs` | Functional health checks (8 checks) | `agent health` |
| `src/manifest.rs` | Model manifests: hash, revision, runtime build, hardware | `agent models manifest`, `agent doctor` |
| `src/sysprobe.rs` | OS, CPU, RAM, GPU, CUDA, WSL, disk, tool versions | `agent doctor`, manifests |
| `src/event_log.rs` | Append-only JSONL log, redacted on write | hooks, MCP, commands |

## Launcher (`launcher.rs`)

`server_args(model, launch, model_file)` is pure and unit-tested. It emits, in order:

```
-m <model_file> --host <host> --port <port> --alias <model.id> -c <context> --jinja --metrics
[-ngl <gpu_layers>] [-t <threads>] [-fa on|off] [-ctk <kv> -ctv <kv>] [--load-mode <mode>]
-np <parallel> [--cache-ram <MiB>] [--reasoning-budget <n>] <extra_args...>
```

- Host and port come from `model.endpoint`. A missing port is `LaunchError::Endpoint`. A non-loopback host is
  `LaunchError::NonLoopbackBind` unless the caller allows a remote bind, so a server never listens on the LAN by accident.
- Flags match llama.cpp b11756: there is no `--no-mmap` in that build (`--load-mode` replaces it), and no
  `--spec-type` unless the user adds it. The profiles set `load_mode = "mmap"`, `parallel = 1` and
  `cache_ram_mib = 1024`; the server's default prompt cache (8 GiB) would starve a 16 GB host.
- Model file and alias are separate argv elements. No shell is involved.

`spawn_server(bin, args, log_file)` appends stdout and stderr to the log file and uses `CREATE_NO_WINDOW` on
Windows. A missing binary is `LaunchError::MissingBinary(path)`.

`wait_until_healthy(health_url, timeout, poll, child)` polls `/health` (2 s per request) until HTTP 200 and
returns the elapsed load time (R-41). It fails fast with `LaunchError::Exited` if the server dies, and with
`NotReady` on timeout.

Open findings (M1 review): `extra_args` is appended after the loopback check and is not validated, so
`--host 0.0.0.0` there re-binds the server (major); no process group on Unix (minor).

## Health checks (`health.rs`)

An HTTP 200 does not prove the model works (§44). `run_checks(provider, model, opts)` runs, in order:

| # | Check | Pass when |
|---|---|---|
| 1 | Health | `/health` is ok. If it fails, every later check is Skip |
| 2 | Models | the server lists `model.id` (Warn if reachable but the id differs) |
| 3 | Completion | `Reply with the single word READY.` returns `READY` (thinking off, 32 tokens, temperature 0) |
| 4 | Streaming | at least 2 deltas; records time to first token (Skip without the capability) |
| 5 | ToolCall | a `get_weather` call whose `city` contains Paris (Warn if malformed) |
| 6 | StructuredOutput | JSON matching a strict schema (`decision` enum, `confidence` in 0..1) |
| 7 | Context | server `n_ctx >= model.context` (Warn if smaller) |
| 8 | Mtp | informational: speculative decoding detected or not |

`opts.quick` runs checks 1 to 3. `summarize` gives Fail if any check failed, else Warn if any warned, else
Pass. The checks never panic. Verified on Windows with Qwen3.5-9B: all checks pass in 2.5 s, 98 ms to first token.

## Manifests (`manifest.rs`)

A manifest records what was actually run, so benchmark results are comparable (§38, R-41).

- `sha256_file(path)`: streaming SHA-256 with a 1 MiB buffer.
- `cached_sha256(cache_path, file)`: cache at `<state>/manifests/hash-cache.json`, keyed by absolute path, valid
  while size and mtime match. Hashing a 6 GB file takes tens of seconds; the cache makes repeat runs fast.
- `parse_llama_version(text)`: reads `version: 11756 (a1b2c3d)` from `--version` output (stdout or stderr; `\r\n` tolerated).
- `build_manifest(input)`: pure. `verified` is `None` when no expected hash is configured, otherwise whether the
  hash matches `source.sha256` (case-insensitive).
- `write_manifest(dir, manifest)`: `<dir>/<slot>.json`, pretty JSON.

## Host probes (`sysprobe.rs`)

Every probe is best effort: a failure becomes `None` or an empty list, never an error or a panic. External commands
run with a 5 s timeout.

- `probe_system()`: OS, kernel, CPU brand and count, total and available memory (`sysinfo`), GPUs, WSL facts.
- `probe_nvidia()`: `nvidia-smi --query-gpu=...` parsed by `parse_nvidia_csv`; CUDA version from plain
  `nvidia-smi` (`parse_cuda_version`).
- `probe_wsl()`: inside WSL (from `/proc/sys/kernel/osrelease`); on Windows, `.wslconfig` limits
  (`parse_wslconfig`) and running distributions (`wsl.exe -l --running -q`, UTF-16LE decoded).
- `free_space(path)`: free bytes on the volume holding `path` (longest mount-point prefix).
- `command_version(bin, args)`: first non-empty line of a tool's version output.

## Event log (`event_log.rs`)

`EventLog::for_workspace(...)` writes to `<workspace>/.local-agent/`: `tool-events.jsonl` (hook decisions) and
`events.jsonl` (everything else).

- `append(event)` redacts the event with the configured redactor, caps every string field at
  `telemetry.max_field_bytes` (`truncate_strings`, char-safe), and writes one line with a single `write_all` in
  append mode. That keeps lines whole when several hook processes append at once.
- Telemetry must not break the agent: I/O errors are returned for the caller to ignore or report, never panicked on.
- With `secrets_redaction = false`, fields are written raw (config-gated; flagged by the redaction review).

## Tests

- `tests/launcher_wait.rs`: a flaky health server (503, then 200), nothing listening (timeout), a child that
  exits while loading, and argv snapshots for a GPU slot, a CPU slot, and `extra_args` placement.
- `src/launcher.rs`: optional flags, endpoint parsing, loopback rules, IPv6, missing binary.
- `tests/health_mock.rs`: each pass, warn and fail path with `MockProvider::scripted`.
- `src/manifest.rs`: known digest, cache hit and miss, verified flag, version parsing.
- `src/sysprobe.rs`: every pure parser, using real `nvidia-smi` output from the target laptop.
- `src/event_log.rs`: truncation at char boundaries. Gap: no test writes an event with a secret and checks the
  file. Redaction on write was verified only by the Windows end-to-end run (the planted `.env` value was absent
  from `tool-events.jsonl`). A unit test is owed.
