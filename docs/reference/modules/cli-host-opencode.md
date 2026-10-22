# Module: harness-cli/host/opencode_v1

The OpenCode adapter and the plugin shim. `opencode_v1.rs` is the only Rust code that knows OpenCode: it maps
OpenCode tool calls and permission requests to host-neutral `ToolRequest`s, answers the shim's hook calls with JSON
verdicts, and generates `opencode.json` from the harness config. The TypeScript shim forwards hooks to
`agent hook <event>` and holds no policy logic.

> **Status:** Built and tested in M1, pinned to OpenCode 1.19.3. Verified end to end on native Windows with
> Qwen3.5-9B ([progress.md](../../status/progress.md)).

## Responsibilities

| File | Public items | What they do |
|---|---|---|
| `crates/harness-cli/src/host/opencode_v1.rs` | `HookEvent`, `HookContext`, `handle`, `map_tool_call`, `map_permission`, `BASH_ALLOW`, `BASH_EXPLORE_ALLOW`, `wildcard_match`, `build_allow_patterns`, `shell_env`, `failure_answer`, `provider_id`, `model_ref`, `generate_opencode_config` | Tool mapping, hook answers, `opencode.json` generation |
| `opencode-plugin/local-agent.ts` | `LocalAgentPlugin` | The TypeScript shim OpenCode loads; forwards hooks to `agent hook <event>` |
| `opencode-plugin/package.json` | | Exact pins: `@opencode-ai/plugin` 1.19.3, `typescript` 5.6.3, `@types/node` 18.19.130 |
| `crates/harness-cli/src/commands/hook.rs` | `run` | Process entry for hooks: stdin to `handle`, answer to stdout, event to the log |

Pinned host: OpenCode 1.19.3 (ADR-0001, R-33.2). On Windows it runs bash-tool commands in PowerShell
(`powershell -NoProfile -Command`) unless `SHELL` names another shell; the policy lexes commands the same way
([core-security.md](core-security.md)). A new OpenCode version means a new adapter module (`opencode_v2.rs`), not
edits scattered across the code.

## Behaviour

### Hook protocol

The shim spawns `agent hook <event> --workspace <dir>`, writes one JSON payload to stdin, and reads one JSON answer
from stdout (20 s timeout, `HOOK_TIMEOUT_MS`). The command always exits 0 and always answers, even with a broken
config. The shim never uses a filesystem root as the workspace; it falls back to the session directory.

| OpenCode hook | CLI event | Rust handler | Answer | If the harness cannot answer |
|---|---|---|---|---|
| `tool.execute.before` | `tool-before` | `tool_before` | `{action: allow\|block, message, class, verdict, rule}` | Blocked (the shim throws) |
| `tool.execute.after` | `tool-after` | `tool_after` | `{output}` when redaction changed it | Output withheld |
| `permission.ask` | `permission` | `permission` | `{status: allow\|deny, reason}` | `deny` (D3) |
| `shell.env` | `shell-env` | `shell_env` | `{set: {NAME: ""}}` blanks secret variables | Environment unchanged; the call is still gated by `tool-before` |
| `event` (6 lifecycle types) | `event` | `session_event` | `{}` | Ignored (telemetry only) |

Only six low-frequency events are forwarded (`session.created`, `session.idle`, `session.error`,
`session.compacted`, `file.edited`, `permission.replied`). Streaming events would spawn a process per token.

When `agent hook` itself cannot build its context (bad config, bad workspace, unreadable payload),
`failure_answer(event, error)` answers: `tool-before` blocks, `permission` denies, `shell-env` still blanks
sensitive variables from its own environment, and `tool-after` and `event` return `{}`.

### Tool mapping (`map_tool_call`, `map_permission`)

OpenCode tool names become host-neutral `ToolRequest`s for `harness-core`:

| OpenCode tool | `ToolKind` |
|---|---|
| `read` | Read |
| `list` | List |
| `glob`, `grep`, `codesearch` | Search (a `glob` or `grep` pattern that could leave the workspace adds its base directory as a path) |
| `edit`, `multiedit` | Edit |
| `write` | Write |
| `patch`, `apply_patch` | Patch, with paths parsed from the patch headers |
| `bash` | Shell (`command`, and `workdir` or `cwd`) |
| `webfetch`, `websearch` | WebFetch, WebSearch |
| `task` | Subagent |
| `todowrite`, `todoread`, `question`, `skill` | Todo |
| `local-agent_<tool>` (MCP) | `Harness(<tool>)`, classed by name in `ToolPolicy`: `git_status`, `git_diff`, `harness_info` and the other read-only tools are READ_ONLY; `run_verification` and the other executing tools are EXECUTE_SAFE (each step is checked inside the tool); `adapter_*` is EXECUTE_SAFE ([core-security.md](core-security.md)) |
| anything else | `Other(name)`: EXECUTE_RISKY, so blocked under the default verdicts |

File paths are collected from every argument name OpenCode uses (`filePath`, `file_path`, `path`, `filepath`).

Permission payloads: `bash` takes the command from metadata, then the pattern, then the title.
`external_directory` becomes a Read of the requested path, so the path guard decides.

### `tool_before`: the gate

1. Map the call and run `ToolPolicy::evaluate` ([core-security.md](core-security.md)).
2. Allow-pattern bypass: OpenCode does not ask the user when a command matches an auto-approved prefix. If the
   command matches one (`pytest*` matching `pytest && curl x | sh`) and the policy says Ask, the verdict becomes
   Deny with rule `allow_pattern_bypass`, and the model is told to run the risky step on its own.
3. The reasons are redacted before they reach the model or the log (a blocked command can echo part of itself,
   such as `unrecognised program <word>`).
4. Decision D3: only Allow passes. Ask is blocked, because under `--auto` OpenCode approves anything not denied.
5. The event is logged with the command redacted.

`BASH_ALLOW` (the generated auto-approve list) holds read-only and build/test prefixes such as `git status*`,
`cat *`, `grep *`, `pytest*`, `cargo test*`. `build_allow_patterns` adds `<safe_command>*` for each
`security.safe_commands` entry. Matching a prefix only skips OpenCode's own prompt. Every call still goes through
`tool_before`, which checks the arguments (so `cat .env` is denied by the `secret_file` rule).
`BASH_EXPLORE_ALLOW` is the read-only subset for the explore, review and deep agents. `wildcard_match` follows
OpenCode's semantics: `*` matches any run of characters, `?` exactly one.

### `tool_after`, `permission`, `shell_env`

- `tool_after` redacts the output when `security.secrets_redaction` is on and logs size, whether anything was
  redacted, and a 400-character preview of the redacted text. MCP results are already redacted by the server,
  because OpenCode does not show MCP output to this hook.
- `permission` maps Allow to `allow`, and Ask or Deny to `deny` (D3).
- `shell_env` returns every non-empty variable with a sensitive name (`is_sensitive_entry`), set to `""`. OpenCode
  merges the answer over the inherited environment, so the secret is removed from the shell (R-27.2).

### `opencode.json` generation (`generate_opencode_config`)

One source of truth: the harness config produces `opencode.json` (C-11).

1. One provider per model slot (`provider_id`: `local-agent-<slot>`, for example `local-agent-main`,
   `local-agent-micro`, `local-agent-deep`), all OpenAI-compatible endpoints, which rule V4 keeps on loopback. `enabled_providers` lists only these, so OpenCode cannot fall back to a hosted model. `small_model` is
   the main model too, so titles and summaries also stay local.
2. `share: disabled`, `autoupdate: false`.
3. Agents:
   - `build`: the main model. Permissions: edit allow, read rules (secret files denied), bash `ask` by default plus
     the allow list, webfetch and websearch from `security.network`, external directories denied. `steps` is
     `max_iterations × 10`.
   - `explore`: read-only subagent (edit denied, read-only bash list, no web).
   - `review` and `deep`: generated when a slot has the `review` or `deep_reasoning` role; they analyse and
     propose, and never edit (§18).
4. MCP: `local-agent` runs `<agent.exe> mcp` over stdio. OpenCode applies `timeout` to every MCP request, so it is
   the sum of the verification step timeouts (steps run in turn), at least the `git_diff` worst case (120 s,
   `MCP_GIT_WORST_CASE_MS`), plus 30 s (`MCP_TIMEOUT_MARGIN_MS`).

### The shim (`local-agent.ts`)

- No policy logic: it forwards payloads and applies answers.
- `agent init` copies it to `.opencode/plugins/local-agent.ts` and replaces `__AGENT_BIN__` with the absolute path
  of `agent.exe`. `LOCAL_AGENT_BIN` overrides that at run time.
- The type import from `@opencode-ai/plugin` is erased at run time. `npm run typecheck` checks the shim against the
  pinned 1.19.3 types.

## Tests

- `crates/harness-cli/tests/hook_fixtures.rs`: 34 payloads through the real binary
  (`every_hook_fixture_row_matches_the_policy`): secret reads via each allowed prefix, `allow_pattern_bypass`,
  Ask-class commands, malformed input, broken config, the `event` hook.
- Unit tests in `opencode_v1.rs`: `wildcard_semantics`, `maps_builtin_tools`, `glob_patterns_that_escape_are_checked`,
  `patch_envelope_paths`, `permission_mapping`, `shell_env_blanks_secrets_only`,
  `shell_env_blanks_every_secret_name_shape`, `ask_verdicts_are_blocked_not_allowed`,
  `failure_answers_fail_closed`, `blocked_message_and_event_do_not_echo_secrets`,
  `mcp_timeout_covers_every_verification_step_in_turn`, `generated_config_is_local_first`.
- End to end on Windows with Qwen3.5-9B: the bug fix run, the `.env` read denial, and MCP `git_status`, `git_diff`
  and `run_verification` calls ([progress.md](../../status/progress.md)).

## Limits

- If the shim's `shell-env` call fails, secrets stay in the shell environment. Commands that dump the environment
  (`env`, `printenv`, `Get-ChildItem env:`) then rely on output redaction alone.
- On Windows, OpenCode must start with `HTTP_PROXY` and `HTTPS_PROXY` cleared, or plugin loading hangs
  ([troubleshooting.md](../../guide/troubleshooting.md)).
- The adapter is pinned to OpenCode 1.19.3; payload shapes of other versions are not handled.

## Related

- Modules: [core-security.md](core-security.md), [cli-commands.md](cli-commands.md),
  [runtime-mcp.md](runtime-mcp.md), [index](README.md)
- Concepts: [architecture.md](../../concepts/architecture.md), [security.md](../../concepts/security.md)
- Decisions: [docs/decisions/](../../decisions/), in particular
  [ADR-0001](../../decisions/0001-rust-core-thin-host-adapter.md)
- Guide: [troubleshooting.md](../../guide/troubleshooting.md)
