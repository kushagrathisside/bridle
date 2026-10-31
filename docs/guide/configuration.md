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

### `[agent]`: loop bounds

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

| Key | Default | Meaning |
|---|---|---|
| `require_worktree` | `true` | Read by the configuration checks only. No code path requires a worktree yet. |
| `network` | `ask` | The verdict for network operations. |
| `destructive_commands` | `ask` | The verdict for destructive operations. |
| `privileged_commands` | `deny` | The verdict for privileged operations. |
| `risky_commands` | `ask` | The verdict for unrecognised programs and arbitrary scripts. |
| `secrets_redaction` | `true` | Redacts secret values from output and logs. |
| `allowed_paths` | `[]` | Extra directories, besides the workspace, that file tools may use. Machine configuration only. |
| `safe_commands` | `[]` | Extra command prefixes, such as `"just test"`, that run without a block. Machine configuration only. |

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
