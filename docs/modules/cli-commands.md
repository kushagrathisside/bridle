# Module: command-line interface (`harness-cli`)

Crate: `crates/harness-cli`, binary `agent`. Wiring only: logic lives in `harness-core` and `harness-runtime`, and
everything OpenCode-specific lives in `src/host/opencode_v1.rs` ([cli-host-opencode.md](cli-host-opencode.md)).

| File | Purpose |
|---|---|
| `src/main.rs` | clap definitions and dispatch |
| `src/app.rs` | `App`: workspace, effective config, validation issues, and the services built from them |
| `src/output.rs` | Status lines for `doctor` and `health` |
| `src/commands/*.rs` | One file per command |

Global options: `--config <file>` (or `LOCAL_AGENT_CONFIG`) adds the highest-precedence config layer;
`--workspace <dir>` sets the repository root (default: the current directory).

## Application context (`app.rs`)

`App::load(config, workspace)` resolves the workspace, loads the layered config and validates it. Validation issues
are kept, not fatal: each command decides. Only an unreadable or unparseable config fails the load.

- `resolve_workspace` canonicalises the path and refuses a filesystem root, because a root workspace would let
  the path guard allow everything.
- `state_dir()` is `<workspace>/.local-agent`. `event_log()` and `policy()` build those services from the config.
- `require_valid_config()` fails with a readable list when the config has errors.

Two commands bypass `App::load`: `hook` (it must answer JSON even when the config is broken) and `init` (it may be
creating the config).

## Commands

| Command | What it does | Exit code |
|---|---|---|
| `agent doctor [--json]` | Read-only report: OS, CPU, RAM, GPU, CUDA, WSL, disks, runtimes, model files, quick endpoint health, OpenCode binary, config issues | Failure only when a configured runtime is missing or the config has errors |
| `agent health [slot] [--quick] [--json]` | Functional checks per slot ([runtime-serving.md](runtime-serving.md)). A slot whose provider cannot be built is one failed check; the other slots still run | Failure if any check fails |
| `agent config show` | The effective config, redacted, and the files it came from | |
| `agent config validate` | Rules V1 to V21 | Failure on any error |
| `agent config paths` | Machine config, repo config, state directory | |
| `agent init [--profile local-only\|development] [--force]` | Writes `local-agent.toml` (never overwritten), `opencode.json`, `.opencode/plugins/local-agent.ts`, and a `.gitignore` entry | |
| `agent init --machine --models-dir D:\models --runtimes-dir D:\runtimes --opencode-bin <exe>` | Writes the machine config | |
| `agent models list` | Configured slots | |
| `agent models serve <slot> [--detach] [--wait-secs 300]` | Starts the slot's server; logs to `.local-agent/logs/<slot>.log`, PID to `.local-agent/servers/<slot>.pid` | The server's exit code when attached |
| `agent models stop <slot>` | Stops a detached server | |
| `agent models manifest [slot] [--no-hash]` | Writes `.local-agent/manifests/<slot>.json` | |
| `agent hook <event>` (hidden) | Plugin bridge: JSON on stdin, JSON on stdout, always exit 0 | 0 |
| `agent mcp` | MCP server on stdio ([runtime-mcp.md](runtime-mcp.md)) | |

### doctor (`commands/doctor.rs`)

Missing optional pieces are warnings, so `doctor` is safe on a bare machine. It warns when the models directory
does not exist or sits on the system drive (D-2), when a model file is missing, and when an endpoint is unreachable.
It checks the OpenCode binary against the pinned version. Verified by hand with missing models and runtimes:
runtimes block, models warn, no panic, exit code 1.

When a launched slot does not answer, `doctor` tries to bind its port and reports a port held by another
program (so `serve` would fail) or one the OS refuses (on Windows, usually an excluded port range; it names the
`netsh` command to list them) (M1 exit item 4).

### init (`commands/init.rs`)

- Profiles are embedded at build time (`include_str!` of `configs/local-only.toml` and `configs/development.toml`),
  so the binary needs no files beside it.
- Generated files are tracked by hash in `.local-agent/generated.json` (the ledger). A generated file the user has
  edited, or one the ledger does not know, is replaced only with `--force`.
- The shim copy gets the absolute path of the `agent` binary baked in, in place of `__AGENT_BIN__`.
- `init --machine` stops with an error when an existing machine config cannot be read, rather than replacing it.
  The ledger is saved even when a later file fails, so the next run recognises what was written.
- Under the AI desktop app (an MSIX package), `%APPDATA%` writes from processes it starts are redirected to
  `%LOCALAPPDATA%\Packages\AI_<id>\LocalCache\Roaming\`. A machine config written that way is invisible to
  programs started from your own terminal. Run `agent init --machine` from your own terminal for a config every
  program sees.

### models (`commands/models.rs`)

- `serve` builds a plan (`plan_serve`: model file, binary, argv) and reports a missing file or binary before spawning.
  With `--detach` it returns once `/health` answers 200 and leaves the server running.
- The record (`.local-agent/servers/<slot>.pid`) holds `pid=` and `bin=`. `stop` signals the PID (`taskkill /T /F`
  on Windows, `kill` elsewhere) only if it is in `1..=i32::MAX` and the running process's executable name matches
  the recorded binary. A stale or reused PID (after a crash or reboot) is reported and its record removed, never
  signalled. `serve` clears a stale record instead of refusing to start.
- `manifest` hashes the model file (cached), reads the runtime version from `<bin> --version`, and probes the host.
- Slot names are checked so they cannot escape the state directory.
- `<bin> --version` runs with a 5 s timeout, so a hung binary cannot hang `manifest` or `doctor`.

### hook and mcp

`commands/hook.rs` reads one payload, calls `host::opencode_v1::handle`, appends the event to the log, and prints
the answer. On any internal error it prints `failure_answer(event, error)`, which blocks policy hooks (fail closed)
and lets telemetry hooks pass. `commands/mcp.rs` builds the tool context (workspace, policy, redactor) and runs the
stdio server.

## Tests

- `tests/hook_fixtures.rs`: 34 black-box cases that run the real `agent hook` binary with OpenCode payloads.
  They cover secret reads through every allowed bash prefix, Ask-class commands, malformed payloads and a missing
  config. They run on Linux and on native Windows.
- Unit tests in `commands/models.rs` (serve plans, the record format and older records, refused PIDs, program
  name matching, a reused PID that is reported and not signalled, state layout), `commands/init.rs` (profiles parse,
  shim placeholder, ledger refuses foreign files, `.gitignore` entries added once), `commands/doctor.rs` (system
  drive, pinned OpenCode version, blocking and non-blocking problems, JSON output), and `app.rs` (root workspace
  rejected, workspace canonicalised).
