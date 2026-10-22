# Configuration

Two files, and a generated one.

1. **Machine config** (`%APPDATA%\local-agent\config.toml` on Windows): where things are on this computer.
   Set it with `agent init --machine --models-dir D:\models --runtimes-dir D:\runtimes --opencode-bin D:\runtimes\opencode\1.19.3\opencode.exe`.
2. **Project config** (`local-agent.toml` at the repo root): created by `agent init --profile local-only`. Edit it per project.
   It arrives with the repository, so it is **untrusted**: it may tune behaviour, but not loosen security (see below).
3. **Generated**: `opencode.json` and `.opencode/plugins/local-agent.ts`. Re-created by `agent init --force`. Do not edit by hand.

Check any change with `agent config validate`. Show the merged result with `agent config show` (secrets redacted).

## Profiles

Shipped profiles live in `configs/`: `local-only.toml` and `development.toml`. Profile names are `local-only`,
`development`, `local-with-jev`, `hybrid`, `benchmark` and `test`. Other values are a parse error.

## Keys you are most likely to change

| Key | Meaning | Default in profile |
|---|---|---|
| `agent.default_model` | Model slot the build agent uses | `main` |
| `models.<slot>.endpoint` | Loopback URL of the server; non-loopback is rejected unless cloud is enabled over HTTPS | `http://127.0.0.1:8001/v1` |
| `models.<slot>.context` | Context the server is started with (`-c`) | 16384 |
| `models.<slot>.launch.load_mode` | `--load-mode` (`auto`, `mmap`, `none`) | `mmap` |
| `models.<slot>.launch.parallel` | Server slots (`-np`); keep 1 on this hardware | 1 |
| `models.<slot>.launch.cache_ram_mib` | Prompt cache cap (`--cache-ram`); the llama.cpp default is 8192 | 1024 |
| `context.hard_limit` | Must not exceed the default model's context (rule V21) | 16384 |
| `security.safe_commands` | Extra command prefixes that may run without asking (machine config or `--config` only; see below) | `[]` |
| `security.network`, `destructive_commands`, `risky_commands` | Verdicts for those classes; `ask` is denied in release 1 (D3) | `ask` |
| `security.privileged_commands` | Verdict for PRIVILEGED commands | `deny` |
| `verification.steps` | Commands `run_verification` may run (see below) | none |

## Running your project's own tests

The policy classifies commands by their program name. Common test runners are built in as safe: `pytest`,
`python -m pytest|unittest|mypy|ruff`, `cargo test|build|check|clippy|fmt`, `go test|build|vet`, `tsc`, `ctest`. Declare
the steps in the project config. Example from `examples/toy-python-repo`:

```toml
[[verification.steps]]
name = "unit tests"
command = "python -m unittest -q"
timeout_ms = 120000
```

For a runner that is not built in (for example `make test`), add its prefix to `security.safe_commands` in the
**machine config**, or in a file you pass with `--config`. A project config may not set it, because a cloned
repository could then mark its own scripts as safe. A safe prefix only upgrades a command that would otherwise be
EXECUTE_RISKY; any part of the command that is more dangerous (for example `; rm -rf x`) is still classified on its
own and denied. Verification steps are classified before they run, in the shell that runs them (`cmd` on Windows).

## Which file may set what

The machine config and an explicit `--config` file are yours, so they may set anything. The project config arrives
with the repository, so `agent` refuses to load it (naming the key) when it tries to:

- set `[paths]` or `host.opencode_bin`;
- name a `runtimes.<name>.server_bin` or `models.<slot>.launch.model_file` that is absolute, uses `..`, a drive, or
  starts with `~`, `$` or `%` (relative names inside the configured directories are fine);
- point `models.<slot>.endpoint` at anything but this machine (`127.0.0.0/8`, `localhost`, `::1`);
- set `models.<slot>.api_key_env`;
- choose the `hybrid` or `local-with-jev` profile, or set any `external.*` key other than `enabled = false`;
- set `security.allowed_paths` or `security.safe_commands`, turn off `secrets_redaction` or `require_worktree`, or
  make a `security` verdict looser than the machine config's (stricter is fine).

The shipped profiles satisfy these rules, so `agent init` output loads as a project config.

## Validation rules

`agent config validate` reports each issue with its rule id. The rules are in
`crates/harness-core/src/config/validate.rs`. Main ones: V4 (endpoint must be loopback unless cloud is enabled
over HTTPS), V12 (no external services under `local-only`), V16 (relative model or runtime paths need the
machine config), V18 (models directory on the system drive warns), V20 (mock provider only under `test`),
V21 (context limit against the default model), V22 (`launch.extra_args` may not contain flags the launcher sets or
that rebind, fetch, serve or write files, such as `--host`, `--port`, `-m`, `--log-file`), V23 (`timeout_ms` within
1 s to 1 h, `retries` at most 10).
