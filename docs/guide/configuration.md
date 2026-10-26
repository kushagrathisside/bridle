# Configuration

This page describes the files that configure the Bridle harness, the keys you are most likely to change, which
file may set what, and the validation rules. It is for an engineer setting up or tuning a project. It assumes you
have installed the `agent` binary as described in [installation.md](installation.md).

> **Status:** Release 1 (native Windows), harness milestones M0 to M2 complete. Behaviour described here is
> implemented and verified on the target machine as of 2026-10-22.

## Configuration files

There are two config files and one set of generated files.

| File | Location | Who writes it | Trust |
|---|---|---|---|
| Machine config | `%APPDATA%\local-agent\config.toml` on Windows | you, with `agent init --machine` | trusted; may set anything |
| Project config | `local-agent.toml` at the repository root | `agent init --profile local-only`, then edited per project | **untrusted**: it arrives with the repository, so it may tune behaviour but not loosen security |
| Generated files | `opencode.json` and `.opencode/plugins/local-agent.ts` | `agent init`; re-created by `agent init --force` | do not edit by hand |

Write the machine config with:

```bat
agent init --machine --models-dir D:\models --runtimes-dir D:\runtimes --opencode-bin D:\runtimes\opencode\1.19.3\opencode.exe
```

A file you pass explicitly with `--config` is treated like the machine config.

## Check and inspect the config

1. Check any change with `agent config validate`. It reports each issue with its rule id (see
   [Validation rules](#validation-rules)).
2. Show the merged result with `agent config show`. Secrets are redacted.

## Profiles

Shipped profiles live in `configs/`: `local-only.toml` and `development.toml`.

Valid profile names are `local-only`, `development`, `local-with-jev`, `hybrid`, `benchmark` and `test`. Any other
value is a parse error.

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
| `security.safe_commands` | Extra command prefixes that may run without asking (machine config or `--config` only) | `[]` |
| `security.network`, `destructive_commands`, `risky_commands` | Verdicts for those classes; `ask` is denied in release 1 (D3) | `ask` |
| `security.privileged_commands` | Verdict for PRIVILEGED commands | `deny` |
| `verification.steps` | Commands the verification tools may run (see below) | none |
| `tools.adapters` | Declarative adapters exposed as `adapter_<name>` tools (machine config or `--config` only) | none |

## Run your project's own tests

The policy classifies commands by their program name. These test runners are built in as safe: `pytest`,
`python -m pytest|unittest|mypy|ruff`, `cargo test|build|check|clippy|fmt`, `go test|build|vet`, `tsc`, `ctest`.

1. Declare the steps in the project config. Example from `examples/toy-python-repo`:

   ```toml
   [[verification.steps]]
   name = "unit tests"
   command = "python -m unittest -q"
   timeout_ms = 120000
   ```

2. Give each step a `kind`: `test` (the default), `lint`, `typecheck`, `build` or `other`.

| Tool | Runs |
|---|---|
| `run_tests` | steps of kind `test` |
| `run_linter` | steps of kind `lint` |
| `run_typechecker` | steps of kind `typecheck` |
| `run_verification` | all steps |

3. To let the model rerun one failing test, set `targets = true` on a test step. `run_tests` then appends test
   files, node ids or test names:

   ```toml
   [[verification.steps]]
   name = "pytest"
   command = "python -m pytest -q"
   targets = true            # run_tests may append e.g. tests/test_calc.py::test_add

   [[verification.steps]]
   name = "ruff"
   command = "ruff check ."
   kind = "lint"
   ```

Targets are checked before use: letters, digits and `_ . / \ : - = , @ +` only, no leading `-`, no `..`, no
absolute or drive path, and the file part must pass the read policy (inside the workspace, not a secret file).

### Runners that are not built in

For a runner that is not built in (for example `make test`), add its prefix to `security.safe_commands` in the
**machine config**, or in a file you pass with `--config`. A project config may not set it, because a cloned
repository could then mark its own scripts as safe.

A safe prefix only upgrades a command that would otherwise be EXECUTE_RISKY. Any part of the command that is more
dangerous (for example `; rm -rf x`) is still classified on its own and denied. Verification steps are classified
before they run, in the shell that runs them (`cmd` on Windows).

## Add tool adapters

`[[tools.adapters]]` entries become `adapter_<name>` tools. They run through the same path as `run_command`, so
they get the same command classification, secret checks and path checks. Declare them only in the machine config
or a `--config` file: a repository may not add tools (enforced in `config::trust`). Rule V25 checks them.

## Which file may set what

The machine config and an explicit `--config` file are yours, so they may set anything. The project config arrives
with the repository, so `agent` refuses to load it, naming the key, when it tries to:

- set `[paths]` or `host.opencode_bin`;
- name a `runtimes.<name>.server_bin` or `models.<slot>.launch.model_file` that is absolute, uses `..`, a drive, or
  starts with `~`, `$` or `%` (relative names inside the configured directories are fine);
- point `models.<slot>.endpoint` at anything but this machine (`127.0.0.0/8`, `localhost`, `::1`);
- set `models.<slot>.api_key_env`;
- choose the `hybrid` or `local-with-jev` profile, or set any `external.*` key other than `enabled = false`;
- set `security.allowed_paths` or `security.safe_commands`, turn off `secrets_redaction` or `require_worktree`, or
  make a `security` verdict looser than the machine config's (stricter is fine);
- declare `[[tools.adapters]]`.

The shipped profiles satisfy these rules, so `agent init` output loads as a project config.

## Validation rules

`agent config validate` reports each issue with its rule id. The rules are in
`crates/harness-core/src/config/validate.rs` (V1 to V29). The main ones:

| Rule | Check |
|---|---|
| V4 | endpoint must be loopback unless cloud is enabled over HTTPS |
| V12 | no external services under `local-only` |
| V16 | relative model or runtime paths need the machine config |
| V18 | models directory on the system drive (warning) |
| V20 | mock provider only under `test` |
| V21 | context limit against the default model |
| V22 | `launch.extra_args` may not contain flags the launcher sets or that rebind, fetch, serve or write files, such as `--host`, `--port`, `-m`, `--log-file` |
| V23 | `timeout_ms` within 1 s to 1 h, `retries` at most 10 |
| V24 | warning: `targets` on a step whose kind is not `test` |
| V25 | an adapter has a bad or duplicate name, a non-bare program, non-plain arguments, or no description |
| V26 | the repeated-command threshold is below 2, or its window is shorter than the count or longer than the 16-call ring |
| V27 | the repeated-edit threshold is below 2 (one edit is progress, not a loop) |
| V28 | the refusal cap is below 1 |
| V29 | `[agent].max_iterations` below 2, or a retry bound below 1: the first step would already block or escalate |

## Workflow and loop limits (M3)

The loop bounds of §17 are in `[agent]`: `max_iterations` (8), `max_implementation_retries` (3),
`max_same_failure` (2: the third verification with the same failure escalates). The thresholds that are new in M3 are
in `[workflow]`:

```toml
[workflow]
enabled = true              # off by default; only the machine config may set it to false
repeated_command_count = 3  # identical commands with no edit between that count as a loop
repeated_command_window = 5 # recent calls in which they are counted (at most 16)
repeated_edit_count = 4     # edits of one file since the last verification
refusal_cap = 3             # loop refusals in a row before the harness stops refusing on loop grounds
task_status = false         # offer the task_status tool to the model
```

`decisions.provider` chooses who decides a stalled loop: `rules` (the default) or `local_lfm`, which asks the micro slot. A
failure to reach the micro slot falls back to the rules. The model answers only ESCALATE, ASK_USER or REPLAN.

A repository may enable the workflow or tune these values; it cannot switch the workflow off. The rule is enforced in
`config/trust.rs`. Read-only calls are never refused by a loop rule. See
[the M3 plan](../status/milestones/m3-state.md#fingerprints-and-loop-rules) for what each rule detects.

## Limits

- `ask` verdicts are denied in release 1 (decision D3).
- Under the desktop app that hosts OpenCode (an MSIX package), a machine config written by a process the app
  started lands in `%LOCALAPPDATA%\Packages\<app>_<id>\LocalCache\Roaming\local-agent\config.toml`, not in
  `%APPDATA%`. Programs started outside the app do not see it. See
  [cli-commands.md](../reference/modules/cli-commands.md).

## Related

- [installation.md](installation.md): build the harness and write the machine config.
- [troubleshooting.md](troubleshooting.md): blocked commands, paths and start-up problems.
- [../concepts/security.md](../concepts/security.md): the policy, redaction and residual risks.
- [../reference/modules/core-config.md](../reference/modules/core-config.md): reference for the config module.
- [../reference/modules/cli-commands.md](../reference/modules/cli-commands.md): reference for the `agent` commands.
- [../status/milestones/m2-tools.md](../status/milestones/m2-tools.md): the M2 deterministic tools, including verification and adapters.
