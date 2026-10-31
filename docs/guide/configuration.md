# Configuration reference

This page lists the configuration files, every key Bridle reads, its default, and the validation rules. The guides show
the settings each task needs. This page lists all of them.

## Files

| File | Location | Created by | Holds |
|---|---|---|---|
| Machine configuration | `%APPDATA%\local-agent\config.toml`, or `config.toml` in the folder named by `LOCAL_AGENT_HOME` | `agent init --machine` | Model and runtime folders, the OpenCode executable, names of credential variables, and external services. |
| Repository configuration | `local-agent.toml` at the repository root | `agent init` (never overwritten) | The agent's limits, workflow, verification steps, context and telemetry. |
| Explicit file | Any path given to `--config`, or in `LOCAL_AGENT_CONFIG` | You | Any setting. Trusted, like the machine configuration. |

`agent init` also writes `opencode.json`, `.opencode/plugins/local-agent.ts` and a `.gitignore`. Do not edit the generated
files. `agent init --force` replaces them. `agent config paths` lists the files Bridle loaded.

## Precedence

From lowest to highest:

1. The built-in defaults shown in the tables below.
2. The machine configuration.
3. The repository configuration.
4. The explicit file.
5. The environment variables `LOCAL_AGENT_MODELS_DIR` and `LOCAL_AGENT_RUNTIMES_DIR`, which replace `paths.models_dir` and
   `paths.runtimes_dir`.

A table merges key by key with the layer below it. A list, such as `verification.steps`, replaces the list below it.

## What a repository file cannot set

A repository's `local-agent.toml` arrives with the code it configures, so Bridle checks it when it loads. Bridle refuses
each of these settings and names the key:

- a `[tools]` table, which declares adapters;
- `[paths]`, or `host.opencode_bin`;
- `workflow.enabled = false`. A repository may enable the workflow but not switch it off;
- `profile = "hybrid"` or `profile = "local-with-jev"`;
- a `server_bin` or `launch.model_file` that is absolute, contains `..` or `:`, or starts with `/`, `\`, `~`, `$` or `%`;
- a model `endpoint` whose host is not this machine (`localhost`, `127.0.0.0/8` or `::1`);
- `models.<slot>.api_key_env`;
- any `external.<name>` key except `enabled = false`;
- a non-empty `security.allowed_paths` or `security.safe_commands`;
- `security.secrets_redaction = false` or `security.require_worktree = false` where the layer below sets `true`;
- a security verdict less strict than the layer below.

The machine configuration and an explicit file may set any of these.

## Profiles

| Profile | What it sets | Who may choose it |
|---|---|---|
| `local-only` (default) | Models on this machine, external services off, `context.hard_limit = 16384`, and a deep reasoning budget of 8192. | Anyone |
| `development` | The same models and security as `local-only`, a deep reasoning budget of 4096, and telemetry fields up to 16384 bytes. | Anyone |
| `local-with-jev` | The optional Jev decision service, off in the shipped file. | Machine configuration only |
| `hybrid` | External model routes, off in the shipped file. | Machine configuration only |

`agent init --profile` accepts all four profiles. A repository file can use only `local-only` or `development`, because
the repository layer refuses the other two. `benchmark` is accepted in a configuration file, but no profile file ships
with it. `test` is the only profile in which `mock` providers are allowed (V20).

## Keys

Each table gives the default the program uses when the key is absent.

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
| `default_model` | `main` | The model slot that runs the agent's own turns. It must be a key of `[models]` (V2). |
| `max_iterations` | 8 | The task is blocked when its iteration count reaches this value. At least 2 (V29). |
| `max_implementation_retries` | 3 | The implementation-retries rule fires when the iteration count goes above this value. At least 1 (V29). |
| `max_same_failure` | 2 | The same-failure rule fires on the identical failure after this many in a row. With the default, the third identical failure fires it. |
| `max_deep_escalations` | 1 | How many escalations to the `deep` slot a task may use. |
| `max_command_retries` | 1 | Declared. No rule reads it yet. |

### `[workflow]`: loop detection

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `false` | Turns on the task journal and the loop rules. Only the machine configuration may set it to `false`. |
| `repeated_command_count` | 3 | Identical non-read-only commands, with no edit between them, that count as a loop. At least 2 (V26). |
| `repeated_command_window` | 5 | How many recent calls are checked for those commands. At least the count, and at most 16, the size of the call ring (V26). |
| `repeated_edit_count` | 4 | Edits to one file since the last verification that count as a loop. At least 2 (V27). |
| `refusal_cap` | 3 | Loop refusals in a row before Bridle stops refusing on loop grounds and shows a notice instead. At least 1 (V28). |
| `task_status` | `false` | Offers the `task_status` tool to the model. |

### `[security]`

## Images in the chat

A chat message can send an image from the workspace: `/image <path> [question]`. The file is read through the same policy
as any file the model could read, must be at most 4 MiB, and must be a PNG, JPEG, GIF or WebP image. The slot that answers
must have `capabilities.vision = true`; otherwise the turn is refused before the model is called. The shipped profiles have
vision off, because none of their models is a vision model here.

## Plugins: tools declared in a manifest

A plugin is a manifest file that declares tools. Each tool runs a fixed program with fixed arguments, and it takes no
arguments from the model (ADR-0013). Plugins are listed in the machine config only; a repository cannot add tools.

```toml
[plugins]
manifests = ["/home/me/plugins/files/plugin.toml"]   # absolute paths (V46)
```

A manifest:

```toml
name = "files"
version = "0.1.0"

[[tools]]
name = "list"
description = "List the repository root"
program = "ls"
args = ["-1"]
declared = "READ_ONLY"      # or EXECUTE_SAFE
```

Each tool is offered as `plugin_<plugin>_<tool>`. Its class is the stricter of the class the manifest declares and the
class the command classifier gives its command. Only read-only and safe-execution tools are offered: a tool whose command
the classifier calls a write, a network use, a destructive command or an interpreter is reported and not offered. A
plugin tool runs through the same checks as `run_command`, in the sandbox. `agent plugins` lists what loaded and why any
tool did not.

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

A verdict of `ask` denies the call. OpenCode's permission check reports `ask` as a denial, so `--auto` cannot approve it.
Verdicts run from `allow`, the least strict, to `deny`, the most strict.

### `[verification]`

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `true` | Turns the verification tools on. |
| `required_for_done` | `true` | Read by the configuration checks only (V15). DONE is not yet gated on it. |
| `default_timeout_ms` | 600000 | The time limit for a step that sets no `timeout_ms`. |
| `steps` | none | The verification steps, declared as `[[verification.steps]]`. |

Each step has these keys:

| Key | Default | Meaning |
|---|---|---|
| `name` | none | An optional label. |
| `command` | required | The command line to run. |
| `timeout_ms` | `default_timeout_ms` | The time limit for this step. |
| `cwd` | the workspace root | The working folder, relative to the workspace root. |
| `kind` | `test` | `test`, `lint`, `typecheck`, `build` or `other`. `run_tests`, `run_linter` and `run_typechecker` choose steps by kind. `run_verification` runs them all. |
| `targets` | `false` | Lets `run_tests` append test targets: files, node ids or test names. Warns unless `kind = "test"` (V24). |

See [run your tests](../guide/run-verification.md) for worked examples.

### `[context]`

| Key | Default | Meaning |
|---|---|---|
| `target_tokens` | 16000 | Shortening of older tool results stops once the context is under this size. |
| `hard_limit` | 24000 | Older results are dropped above this size. It must be at least `target_tokens` (V9) and no larger than the default model's context (V21). The shipped `local-only` file sets 16384. |
| `compact_tool_outputs` | `true` | Turns shortening and dropping on. When `false`, every result is kept whole. |

The most recent results are kept whole while the context fits under the hard limit. Verification results older than the
recent window keep only their failure lines.

### `[telemetry]`

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `true` | Writes event records as JSON lines in the state folder. |
| `destination` | `local` | The only destination. |
| `max_field_bytes` | 2048 | The longest text kept from one field, such as a tool output preview. |

### `[decisions]` and `[routing]`

| Key | Default | Meaning |
|---|---|---|
| `decisions.provider` | `rules` | Who decides a stalled loop: `rules`; `local_lfm`, which uses the micro model when `routing.enable_micro_decisions` is on; or `jev`, which needs `external.jev.enabled` (V11). |
| `decisions.fallback` | `rules` | Used when the primary decider fails or returns an invalid decision. Any other value is a warning (V19). |
| `routing.enable_micro_decisions` | `false` | Lets the micro model decide stalled loops. Choosing `local_lfm` without it is a warning (V40). |
| `routing.enable_deep_escalation` | `false` | Lets an ESCALATE decision reach the `deep` slot. It needs a model with the role `deep_reasoning` (V39). |
| `routing.escalation_threshold` | 0.75 | The confidence an ESCALATE decision needs before the `deep` slot is asked. A repeated failure gets 0.75 at the third identical failure, 0.875 at the fourth and 0.9375 at the fifth. A model's own answer has no confidence and always goes to the `deep` slot. Between 0 and 1 (V10). |

### `[subagents]`

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `false` | Offers the `subagent` tool and enables `agent subagent`. |
| `slot` | `micro` | The model slot that answers. It must be defined under `[models]` (V37). |
| `max_input_bytes` | 4096 | Bytes read from each input. Between 256 and 65536 (V38). |
| `max_inputs` | 4 | Inputs per call. Between 1 and 8 (V38). |

### `[external]`

| Key | Default | Meaning |
|---|---|---|
| `jev.enabled` | `false` | Turns on the Jev decision service. It needs an `endpoint` (V30). |
| `jev.endpoint` | none | `http://` only for a loopback host. Any other host must use `https://` (V31, V32). |
| `jev.api_key_env` | none | The name of the environment variable that holds the key. Required for a non-loopback endpoint (V33). |
| `jev.timeout_ms` | 5000 | The time limit for one decision. Between 100 and 60000 (V34). |
| `cloud_models.enabled` | `false` | Lets a model endpoint on another host use `https://` (V4). External services are not part of the verified release. See the [project status](../project/status.md). |

### `[paths]` and `[host]`: machine settings

| Key | Default | Meaning |
|---|---|---|
| `paths.models_dir` | none | The folder that holds model files. A relative `launch.model_file` resolves against it (V16). `agent doctor` reports it when unset. |
| `paths.runtimes_dir` | none | The folder that holds inference runtimes. A relative `server_bin` resolves against it (V16). |
| `host.opencode_bin` | none, so `opencode` is looked up on `PATH` | The path to the OpenCode executable. |
| `host.opencode_version` | `1.19.3` | The OpenCode version Bridle is pinned and tested against. |

### `[runtimes.<name>]`

| Key | Default | Meaning |
|---|---|---|
| `kind` | required | `llama_cpp` for the upstream `llama-server`, or `prism_llama_cpp` for the PrismML fork. The ternary formats `PTQ1_0` and `PQ2_0` need the fork (V7). |
| `server_bin` | required | The path to the server executable, relative to `paths.runtimes_dir`. |
| `build` | none | The build name the runtime reports, such as `b11756`. |

### `[models.<slot>]`

| Key | Default | Meaning |
|---|---|---|
| `provider` | required | `llama_cpp`, `prism_bonsai`, `openai_compatible`, `litellm` or `mock`. `mock` is for tests only (V20). |
| `id` | required | The model name the server reports. For `litellm`, the route the gateway serves. |
| `display_name` | none | A name shown in output. |
| `role` | required | `engineer`, `decision`, `deep_reasoning`, `review` or `external`. |
| `endpoint` | required | The address of the server, such as `http://127.0.0.1:8001/v1`. A plain `http://` address must be loopback (V3, V4). |
| `api_key_env` | none | The name of an environment variable that holds the key. Never the key itself (V8). Machine configuration only. |
| `context` | required | The context size the server runs with. It must be above zero and above `max_output_tokens` (V5). |
| `max_output_tokens` | 4096 | The longest reply. |
| `timeout_ms` | 300000 | The time limit for one request. Between 1000 and 3600000 (V23). |
| `retries` | 1 | Retries of a failed request. At most 10 (V23). |
| `hardware` | required | `gpu`, `cpu` or `remote`. |
| `kv_cache` | none | The KV cache type, such as `q8_0`. An unrecognised name is a warning (V14). |
| `mtp` | `auto` | Multi-token prediction: `auto` uses it when `agent health` detects it, `on` or `off`. |
| `sampling` | server defaults | `temperature` (0 to 2), `top_p` (above 0, up to 1), `top_k`, `min_p` (0 to 1) and `repeat_penalty`. An unset value uses the server's default (V13). |
| `reasoning` | `enabled = false` | `enabled` sends the thinking switch to the server. `budget_tokens` sets the thinking budget. Unset means the server's default. |
| `capabilities` | see text | `tool_calling`, `vision`, `reasoning`, `structured_output`, `streaming`, `long_context`, `local` and `gpu`. All default to `false` except `streaming` and `local`, which default to `true`. |
| `source` | none | Where the weights came from: `repo`, `revision`, `file`, `sha256`, `quantization` and `license`. `agent models manifest` checks the file against `sha256`. |

### `[models.<slot>.launch]`

| Key | Default | Meaning |
|---|---|---|
| `runtime` | required | A key of `[runtimes]` (V6). |
| `model_file` | required | The path to the weights. A relative path resolves against `paths.models_dir` (V16). |
| `gpu_layers` | none | The number of layers offloaded to the GPU. |
| `threads` | none | The number of CPU threads. |
| `flash_attn` | none | Turns flash attention on or off. |
| `load_mode` | none | `auto` lets the server choose, `mmap` memory-maps the weights, and `none` reads them into memory. |
| `parallel` | 1 | The number of server slots. |
| `cache_ram_mib` | none | The size of the prompt cache, in MiB. |
| `extra_args` | `[]` | Extra server flags. A flag that sets the listener, model, alias, context, key, served or written paths is refused (V22). |

## Validation

`agent config validate` prints one line per issue, in the form `✗ [V<n>] <path> - <message>` for an error or
`⚠ [V<n>] <path> - <message>` for a warning. It exits with code 1 when there is an error. Warnings alone exit with code 0.

The rules:

| Rule | Severity | Checks |
|---|---|---|
| V1 | Error | `models` is empty |
| V2 | Error | `agent.default_model` is not a key of `models` |
| V3 | Error | an endpoint does not start with `http://` or `https://`, or has an empty host |
| V4 | Error | an endpoint host is not loopback (`127.0.0.0/8`, `localhost`, `[::1]`) unless `external.cloud_models.enabled` and the scheme is `https`; or `capabilities.local == false` while `external.cloud_models.enabled == false` (local-first) |
| V5 | Error | `context == 0`, or `max_output_tokens >= context` |
| V6 | Error | `launch.runtime` is not a key of `runtimes` |
| V7 | Error | provider `prism_bonsai` with a launch runtime whose kind is not `prism_llama_cpp`; or a ternary quantisation (`PTQ1_0`/`PQ2_0` in `source.quantization` or in the `launch.model_file` name, case-insensitive) launched with a `llama_cpp` runtime |
| V8 | Error | `api_key_env` (models and `external.jev`) does not match `^[A-Z_][A-Z0-9_]*$` (it probably contains a secret) |
| V9 | Error | `context.target_tokens > context.hard_limit` |
| V10 | Error | `routing.escalation_threshold` outside `[0, 1]` |
| V11 | Error | `decisions.provider` or `decisions.fallback` is `jev` while `external.jev.enabled == false` |
| V12 | Error | `external.jev.enabled` or `external.cloud_models.enabled` while `profile == "local-only"` |
| V13 | Error | `sampling.temperature` outside `[0, 2]`, `top_p` outside `(0, 1]`, `min_p` outside `[0, 1]` |
| V14 | Warning | `kv_cache` not one of `f16, bf16, q8_0, q4_0, q4_1, q5_0, q5_1, iq4_nl` |
| V15 | Warning | `verification.enabled && required_for_done && steps.is_empty()` — DONE can never be verified |
| V16 | Error | a launch `model_file` is relative and `paths.models_dir` is `None` (same for runtime `server_bin` / `paths.runtimes_dir`) |
| V17 | Warning | two model slots share the same endpoint |
| V18 | Warning | `paths.models_dir` is on the system drive (`env.system_drive`, compare case-insensitively on the drive prefix such as `C:`) |
| V19 | Warning | `decisions.fallback != rules` — the fallback should be deterministic |
| V20 | Error | provider `mock` in any profile other than `test` |
| V21 | Error | `context.hard_limit > models[agent.default_model].context` (skipped when the default model is missing) |
| V22 | Error | `launch.extra_args` contains a reserved flag ([`super::RESERVED_LAUNCH_FLAGS`]: listener, model, alias, context, key, served or written paths), as `--flag` or `--flag=value` |
| V23 | Error | `timeout_ms` outside `1_000..=3_600_000`, or `retries > 10` |
| V24 | Warning | a verification step sets `targets = true` but its `kind` is not `test` |
| V25 | Error | an adapter (`[[tools.adapters]]`) has a bad or duplicate name, a non-bare program, non-plain arguments, or no description |
| V26 | Error | a repeated-command threshold is below 2, or its window is shorter than the count or longer than the call ring |
| V27 | Error | the repeated-edit threshold is below 2 (one edit is progress, not a loop) |
| V28 | Error | the refusal cap is below 1 |
| V29 | Error | an `[agent]` loop bound would block the task at once: `max_iterations` below 2, or a retry bound below 1 |
| V30 | Error | `external.jev.enabled` without an `endpoint` |
| V31 | Error | `external.jev.endpoint` is not an http(s) URL with a host |
| V32 | Error | a non-loopback Jev endpoint over plain `http://` |
| V33 | Error | a non-loopback Jev endpoint without `api_key_env` |
| V34 | Error | `external.jev.timeout_ms` outside 100 to 60000 |
| V35 | Error | credentials in the Jev endpoint URL |
| V36 | Error | a remote LiteLLM gateway without `api_key_env` |
| V37 | Error | `subagents.enabled` with a `slot` that is not defined under `[models]` |
| V38 | Error | `subagents` limits out of range: `max_input_bytes` outside 256 to 65536, or `max_inputs` outside 1 to 8 |
| V39 | Error | `routing.enable_deep_escalation` without a model whose role is `deep_reasoning` |
| V40 | Warning | `decisions.provider = "local_lfm"` while `routing.enable_micro_decisions` is off |
