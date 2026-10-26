# Module: harness-cli/commands

The `agent` binary: clap definitions, the per-invocation `App` context, and one module per command. It is wiring
only. Logic lives in `harness-core` and `harness-runtime`, and everything OpenCode-specific lives in
`src/host/opencode_v1.rs` ([cli-host-opencode.md](cli-host-opencode.md)). Commands must not hold policy rules of
their own.

> **Status:** Built and tested in M1; `agent mcp` serves the M2 tools and adapters. Release 1 runs on native
> Windows.

## Responsibilities

| File | Public items | What they do |
|---|---|---|
| `src/main.rs` | `Cli`, `Command` | clap definitions and dispatch |
| `src/app.rs` | `App` (`load`, `require_valid_config`, `has_config_errors`, `state_dir`, `event_log`, `policy`), `resolve_workspace` | Workspace, effective config, validation issues, and the services built from them |
| `src/output.rs` | `Mark` | Status lines for `doctor` and `health` |
| `src/commands/*.rs` | `run` in each of `config`, `doctor`, `health`, `hook`, `init`, `mcp`, `models` | One file per command |

Global options: `--config <file>` (or `LOCAL_AGENT_CONFIG`) adds the highest-precedence config layer;
`--workspace <dir>` sets the repository root (default: the current directory).

| Command | What it does | Exit code |
|---|---|---|
| `agent doctor [--json]` | Read-only report: OS, CPU, RAM, GPU, CUDA, WSL, disks, runtimes, model files, quick endpoint health, OpenCode binary, config issues | Failure only when a configured runtime is missing or the config has errors |
| `agent health [slot] [--quick] [--json]` | Functional checks per slot ([runtime-serving.md](runtime-serving.md)). A slot whose provider cannot be built is one failed check; the other slots still run | Failure if any check fails |
| `agent config show` | The effective config, redacted, and the files it came from | |
| `agent config validate` | Rules V1 to V35 | Failure on any error |
| `agent config paths` | Machine config, repository config, state directory | |
| `agent init [--profile local-only\|development] [--force]` | Writes `local-agent.toml` (never overwritten), `opencode.json`, `.opencode/plugins/local-agent.ts`, and a `.gitignore` entry | |
| `agent init --machine --models-dir D:\models --runtimes-dir D:\runtimes --opencode-bin <exe>` | Writes the machine config | |
| `agent models list` | Configured slots | |
| `agent models serve <slot> [--detach] [--wait-secs 300]` | Starts the slot's server; logs to `.local-agent/logs/<slot>.log`, PID to `.local-agent/servers/<slot>.pid` | The server's exit code when attached |
| `agent models stop <slot>` | Stops a detached server | |
| `agent models manifest [slot] [--no-hash]` | Writes `.local-agent/manifests/<slot>.json` | |
| `agent hook <event>` (hidden) | Plugin bridge: JSON on stdin, JSON on stdout, always exit 0 | 0 |
| `agent mcp` | MCP server on stdio ([runtime-mcp.md](runtime-mcp.md)); refuses to start when the config has errors | |

## Behaviour

### Application context (`app.rs`)

`App::load(config, workspace)` resolves the workspace, loads the layered config and validates it. Validation issues
are kept, not fatal: each command decides. Only an unreadable or unparseable config fails the load.

1. `resolve_workspace` canonicalises the path and refuses a filesystem root, because a root workspace would let the
   path guard allow everything.
2. `state_dir()` is `<workspace>/.local-agent`. `event_log()` and `policy()` build those services from the config.
3. `require_valid_config()` fails with a readable list (rule, path, message) when the config has errors.

Two commands do not go through the `App::load` call in `main`: `hook` (it must answer JSON even when the config is
broken, so it calls `App::load` itself and turns any failure into an answer) and `init` (it may be creating the
config; it loads the config after writing it).

### doctor (`commands/doctor.rs`)

- Missing optional pieces are warnings, so `doctor` is safe on a bare machine. It warns when the models directory
  does not exist or sits on the system drive (D-2), when a model file is missing, and when an endpoint is
  unreachable. It checks the OpenCode binary against the pinned version.
- Verified by hand with missing models and runtimes: runtimes block, models warn, no panic, exit code 1.
- When a launched slot does not answer, `doctor` tries to bind its port and reports a port held by another program
  (so `serve` would fail) or one the OS refuses (on Windows, usually an excluded port range; it names the `netsh`
  command to list them) (M1 exit item 4).

### init (`commands/init.rs`)

- Profiles are embedded at build time (`include_str!` of `configs/local-only.toml` and `configs/development.toml`),
  so the binary needs no files beside it.
- Generated files are tracked by hash in `.local-agent/generated.json` (the ledger). A generated file the user has
  edited, or one the ledger does not know, is replaced only with `--force`.
- The shim copy gets the absolute path of the `agent` binary baked in, in place of `__AGENT_BIN__`.
- `init --machine` stops with an error when an existing machine config cannot be read, rather than replacing it.
- The ledger is saved even when a later file fails, so the next run recognises what was written.
- Under the desktop app that hosts OpenCode (an MSIX package), `%APPDATA%` writes from processes it starts are
  redirected to `%LOCALAPPDATA%\Packages\<app>_<id>\LocalCache\Roaming\`. A machine config written that way is
  invisible to programs started from your own terminal. Run `agent init --machine` from your own terminal for a
  config every program sees.

### models (`commands/models.rs`)

1. `serve` builds a plan (`plan_serve`: model file, binary, argv) and reports a missing file or binary before
   spawning. With `--detach` it returns once `/health` answers 200 and leaves the server running.
2. The record (`.local-agent/servers/<slot>.pid`) holds `pid=` and `bin=`. `stop` signals the PID (`taskkill /T /F`
   on Windows, `kill` elsewhere) only if it is in `1..=i32::MAX` and the running process's executable name matches
   the recorded binary. A stale or reused PID (after a crash or reboot) is reported and its record removed, never
   signalled. `serve` clears a stale record instead of refusing to start.
3. `manifest` hashes the model file (cached), reads the runtime version from `<bin> --version`, and probes the host.
4. Slot names are checked so they cannot escape the state directory.
5. `<bin> --version` runs with a 5 s timeout, so a hung binary cannot hang `manifest` or `doctor`.

### hook and mcp

`commands/hook.rs` reads one payload, calls `host::opencode_v1::handle`, appends the event to the log, and prints
the answer. On any internal error, including a config with errors, it prints `failure_answer(event, error)`, which
blocks policy hooks (fail closed) and lets telemetry hooks pass. `commands/mcp.rs` requires a valid config, builds
the tool context (workspace, config, verification, policy, redactor), adds adapter tools, and runs the stdio server.

## Tests

- `tests/hook_fixtures.rs`: 34 black-box rows (`every_hook_fixture_row_matches_the_policy`) that run the real
  `agent hook` binary with OpenCode payloads. They cover secret reads through every allowed bash prefix, Ask-class
  commands, malformed payloads and a missing config. They run on Linux and on native Windows.
- `commands/models.rs`: serve plans, the record format and older records, refused PIDs
  (`records_never_hold_a_process_group_or_wrapped_pid`), program name matching, a reused PID that is reported and
  not signalled (`a_reused_pid_is_reported_not_signalled`), `slot_names_cannot_escape_the_state_directory`, state
  layout.
- `commands/init.rs`: `profiles_parse`, `shim_has_placeholder`, `ledger_refuses_foreign_files`,
  `an_unreadable_machine_config_is_an_error_not_a_blank_slate`, `the_ledger_is_saved_when_a_later_host_file_fails`,
  `gitignore_entries_added_once`.
- `commands/doctor.rs`: system drive, pinned OpenCode version, blocking and non-blocking problems, port states, JSON
  output.
- `commands/health.rs`: `unreachable_endpoint_is_a_failed_health_check`,
  `unset_api_key_fails_without_contacting_the_endpoint`, `fake_server_passes_quick_checks`.
- `commands/config.rs`: `validation_lines_do_not_echo_secret_values`.
- `app.rs`: `filesystem_root_is_rejected`, `workspace_is_canonicalised`.

## Limits

- A machine config written by `agent init --machine` from inside the MSIX-hosted desktop app is redirected and
  invisible to other programs (see init above).
- Detached servers are tracked by a PID record only; a server started some other way is not seen by `stop`.

## Related

- Modules: [cli-host-opencode.md](cli-host-opencode.md), [core-config.md](core-config.md),
  [runtime-serving.md](runtime-serving.md), [runtime-mcp.md](runtime-mcp.md), [index](README.md)
- Concepts: [architecture.md](../../concepts/architecture.md), [security.md](../../concepts/security.md)
- Decisions: [docs/decisions/](../../decisions/)
- Guide: [installation.md](../../guide/installation.md), [configuration.md](../../guide/configuration.md),
  [troubleshooting.md](../../guide/troubleshooting.md)
