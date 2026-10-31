# Configuration

This page describes the files that configure the Bridle harness, the keys you are most likely to change, which
file may set what, and the validation rules. It is for an engineer setting up or tuning a project. It assumes you
have installed the `agent` binary as described in [installation.md](installation.md).

> **Status:** Release 1 (native Windows). The behaviour described here is implemented and verified on the target
> machine as of 2026-10-22.

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
| `security.network`, `destructive_commands`, `risky_commands` | Verdicts for those classes; `ask` is denied in release 1 | `ask` |
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
`crates/harness-core/src/config/validate.rs` (V1 to V41). The main ones:

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
| V41 | `memory.file` is not a relative path inside the repository, or `memory.max_bytes` is outside 256 to 16384 |
| V42 | `external.laya` is enabled without an `endpoint`, or the endpoint is not `http://` to a loopback host |
| V43 | `external.laya.timeout_ms` is outside 100 to 60000 |
| V44 | warning: `routing.auto_slot` is on while `external.laya` is off, so requests the rules cannot place go to `main` |

## Workflow and loop limits

The loop bounds are in `[agent]`: `max_iterations` (8), `max_implementation_retries` (3),
`max_same_failure` (2: the third verification with the same failure is a loop, and the rule fires there). The loop thresholds are
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

`[routing] escalation_threshold` (0.75) is the confidence an ESCALATE decision needs before the deep slot is asked. It
applies only when `[routing] enable_deep_escalation` is on. The loop rules give a repeated failure a confidence that grows
with the number of identical failures in a row: 0.5 for two, 0.75 for three, 0.875 for four and 0.9375 for five. The curve
is a fixed choice, not a measured probability. With the default `max_same_failure = 2`, the default threshold is met at the
third identical failure, and the deep slot is asked at that failure. Two things follow from this:

- Lowering `max_same_failure` lowers the confidence at the rule's first firing. With `max_same_failure = 1` the first firing
  has confidence 0.5, so the default threshold does not send that loop to the deep slot. Set the threshold to match.
- A threshold above 0.75 holds the deep slot back until more identical failures have happened. The agent is refused after
  the third failure, so it reaches more failures only once `refusal_cap` lets a call through.

A loop rule is evaluated when a verification fails. An escalation taken there is recorded, and its answer applies to the
calls after it until a verification passes or the task is reset.

A repository may enable the workflow or tune these values; it cannot switch the workflow off. The rule is enforced in
`config/trust.rs`. Read-only calls are never refused by a loop rule. The loop rules are described in the
[orchestrator module reference](../reference/modules/orchestrator.md).

## Context budget

The `[context]` section sets the budget for tool results in each model request. The rules are described in the
[context module reference](../reference/modules/context.md).

| Key | Default | Meaning |
|---|---|---|
| `context.target_tokens` | 16000 | Older results are compacted until the messages are under this size. |
| `context.hard_limit` | 24000 in code; 16384 in the shipped profile | Older read-only results are dropped above this size, then large results are compacted. Must not exceed the default model's context (rule V21). |
| `context.compact_tool_outputs` | true | `false` turns compaction and dropping off. Requests are still measured. |

Sizes are estimated as bytes divided by four, over the messages of a request only. The system prompt and the tool
definitions are not counted, so leave room for them: one live request of 30,284 bytes of messages was 20,082 tokens.
Use a lower `hard_limit` for numbered or repetitive output.

Each request writes one `context` record to `.local-agent/events.jsonl`, with `tool_results`, `changed`, `bytes_before`,
`bytes_after`, `target_bytes` and `hard_bytes`.

## Subagents: a small model reads a few files

The subagent tier lets a small local model read a few files, or condense one text, and answer one question with cited
evidence. It is off by default. The design is in [ADR-0009](../decisions/0009-subagents-are-bounded-readers.md).

| Key | Default | Meaning |
|---|---|---|
| `subagents.enabled` | false | Offers the `subagent` tool to the model, and makes `agent subagent` run. |
| `subagents.slot` | `micro` | The model slot that answers. It must be defined under `[models]` (V37). |
| `subagents.max_input_bytes` | 4096 | Bytes read from each input before the model sees it (V38: 256 to 65536). |
| `subagents.max_inputs` | 4 | Inputs per call (V38: 1 to 8). |

Inputs go through the tool policy first, so a secret file is refused, and each file is redacted before the model sees it.
The answer is checked against a fixed shape; references it cannot verify are dropped and counted. Treat an answer as a
reading, and check the cited lines.

## Sandbox: commands run confined (Linux)

Commands the harness runs (`run_command`, the verification steps, `run_tests`, `run_linter`, `run_typechecker`) run under
a Landlock sandbox on Linux (ADR-0016). It is on by default on Linux and off on other systems, which have no sandbox yet.

| Key | Default | Meaning |
|---|---|---|
| `sandbox.enabled` | true on Linux | Commands run confined. Only the machine config may set it to false. |
| `sandbox.network` | false | Commands may open network connections. |

What a confined command can reach: writes in the repository, `/tmp`, `/var/tmp` and `/dev/null`; reads of the repository,
the system directories and `~/.cargo` and `~/.rustup`. The rest of the home directory, including SSH keys, is unreadable.
Git inside the sandbox runs without the user's global configuration. If the kernel cannot apply the sandbox, the command is
refused and the refusal says why; it is never run unconfined. Network access needs ABI 4 (Linux 6.7 or later) to be denied.

## Routing: which model answers a request

With `[routing] auto_slot = true`, each chat request sent to the default slot is routed before the turn. The rules place
the request first: an explicit `/deep` or `/main` at the start; a short request with no hard word goes to `main`. A
request the rules cannot place goes to the decision model, Laya, which answers `MAIN` or `DEEP`. `deep` is used only when
Bridle reports the deep slot running; otherwise the request goes to `main`, and the transcript says why.

Laya is a local decision server (ADR-0015). Start it on this machine, then enable it in an explicit config file (external
providers are machine settings, so the repository cannot set them):

```toml
[routing]
auto_slot = true

[external.laya]
enabled = true
endpoint = "http://127.0.0.1:8000"
timeout_ms = 5000
```

`agent route "<text>"` shows where a request would go and why, without starting a turn. If the decision server cannot be
reached or answers outside its choices, the request goes to `main` and the reason says so.

## Repository memory: AGENTS.md

A repository can keep instructions for the agent in a file, `AGENTS.md` by default: how to build and test, what not to
touch, the house style. Bridle adds the file to the system prompt of every request, after the fixed instructions and
before the task state. It is on by default, and a missing file adds nothing.

| Key | Default | Meaning |
|---|---|---|
| `memory.enabled` | true | Whether the file is read. |
| `memory.file` | `AGENTS.md` | The file, relative to the repository. It must stay inside the repository (V41). |
| `memory.max_bytes` | 4096 | The most bytes that go into the prompt (V41: 256 to 16384). Longer text is cut and says so. |

What the model is told. The block starts with a line that says the text is guidance from the repository, not a
permission, and that every tool call is still checked. The file cannot widen the policy: a line that asks for a secret
or a blocked command is refused like any other request. The file is read through the tool policy, so a secret file
named as memory (for example `.env`) is refused and nothing is added, and the refusal is logged. Secret values in the
file are redacted before they reach the prompt.

Budget. 4,096 bytes is about 1,300 tokens at three characters per token. On the shipped 16K slot the tool descriptions
take about 3,600 tokens; a larger memory file reduces the room for the conversation, so keep the file short.

The chat and the OpenCode hook read the same file and add the same block, through the same function, and a refused memory
file is logged the same way from both.

## Limits

- `ask` verdicts are denied in release 1.
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
- [architecture](../concepts/architecture.md#mcp-tools): the MCP tools, including verification and adapters.
