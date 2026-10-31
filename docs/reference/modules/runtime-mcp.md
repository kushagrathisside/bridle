# Module: harness-runtime/mcp

The MCP server that `agent mcp` runs on stdio, and the deterministic tools it serves to OpenCode. It owns the
JSON-RPC 2.0 framing, tool dispatch, redaction of every result, and the bounded process runner the tools share.
OpenCode lists the tools as `local-agent_<tool>`, and every call still passes the `tool-before` hook first
([cli-host-opencode.md](cli-host-opencode.md)). The tools never decide policy themselves; they ask `ToolPolicy`
([core-security.md](core-security.md)).

> **Status:** Built and tested. The server provides the repository, verification, failure, code, dependency, artifact and
> command tools, and the configured adapters.

## Responsibilities

| File | Public items | What they do |
|---|---|---|
| `mcp/mod.rs` | re-exports `PROTOCOL_VERSIONS`, `Server`, `ServerInfo`, `McpTool`, `ToolContext`, `ToolOutput`, `default_tools`, `adapter_tools` | Module surface |
| `mcp/protocol.rs` | `Server` (`new`, `serve`, `handle_line`), `ServerInfo`, `PROTOCOL_VERSIONS` | JSON-RPC 2.0 over newline-delimited stdio; dispatch; redaction of every result |
| `mcp/tools/mod.rs` | `McpTool`, `ToolContext`, `ToolOutput`, `default_tools`, `parse_porcelain_v2`, `parse_shortstat`, `GitStatus`, `DiffStat` | `git_status`, `git_diff`, `run_verification`, `harness_info`; the bounded process runner `run_child` |
| `mcp/tools/history.rs` | `parse_log`, `is_plain_revision`, `Commit` | `git_history`, `git_log_range` |
| `mcp/tools/verify.rs` | `RunKindTool` | `run_tests`, `run_linter`, `run_typechecker` |
| `mcp/tools/failures.rs` | `describe` | `parse_test_failure` (parsing is `harness_core::failures`) |
| `mcp/tools/code.rs` | tool structs | `repo_map`, `file_summary`, `symbol_search`, `find_references` (syntax analysis is `crate::code`, tree-sitter, ADR-0003) |
| `mcp/tools/deps.rs` | `DependencySearchTool` | `dependency_search` (parsing is `crate::code::deps`) |
| `mcp/tools/artifacts.rs` | tool structs | `inspect_artifact`, `summarize_experiment`, `compare_experiments` (summaries are `crate::artifacts`) |
| `mcp/tools/run.rs` | `RunCommandTool`, `InspectConfigTool` | `run_command`, `inspect_config` |
| `mcp/tools/adapters.rs` | `AdapterTool`, `adapter_tools` | One `adapter_<name>` tool per `[[tools.adapters]]` entry |
| `crates/harness-cli/src/commands/mcp.rs` | `run` | Requires a valid config, builds the `ToolContext` (workspace, config, verification, policy, redactor), adds adapter tools, and serves |

`default_tools()` order, as `tools/list` reports it: `git_status`, `git_diff`, `git_history`, `git_log_range`,
`run_verification`, `run_tests`, `run_linter`, `run_typechecker`, `parse_test_failure`, `repo_map`, `file_summary`,
`symbol_search`, `find_references`, `dependency_search`, `inspect_artifact`, `summarize_experiment`,
`compare_experiments`, `run_command`, `inspect_config`, `harness_info`. Adapter tools follow.

## Behaviour

### Protocol (`protocol.rs`)

1. Methods: `initialize` (echoes a supported protocol version: 2025-06-18, 2025-03-26, 2024-11-05; otherwise the
   newest), `notifications/initialized`, `ping`, `tools/list`, `tools/call`. Anything else is -32601.
2. Input is bounded. A line longer than 4 MiB (`MAX_LINE_BYTES`) is discarded and answered with a -32700 parse
   error (id null), and the server keeps serving. Invalid UTF-8 or JSON is -32700.
3. Notifications get no reply, even when their envelope is invalid. Requests with a bad envelope get -32600, and
   so do JSON-RPC batches.
4. An unknown tool name is a -32602 error, and the name is redacted in it.
5. A panicking tool is caught (`catch_unwind`) and reported as `isError: true` without crashing the server.
6. Every tool result, text and structured content alike, passes through the redactor before it is written. The
   after-hook does not see MCP output in OpenCode 1.19.3, so this is the only redaction step for these tools.
7. The server is single-threaded, so every tool is bounded in time and output (below).

### `git_status`

`git status --porcelain=v2 --branch -z`, parsed by the pure `parse_porcelain_v2` into branch, upstream, ahead and
behind counts, and staged, unstaged, untracked and conflicted paths. Text and structured lists show at most 50 paths
each (`LIST_LIMIT`), with `<list>_count` holding the full count. Renames do not reveal the original path. Outside a
repository the call is an error.

### `git_diff`

Arguments: `staged`, `paths`, `stat_only`, `max_bytes` (default 60 000, clamped to 1 000 000, `MAX_DIFF_BYTES`).

1. Each requested path must pass the read policy.
2. The changed names in the scope (the paths, or `.`) are listed with `git diff --name-only -z --relative`, read up
   to 4 MiB (`NAME_LIST_LIMIT_BYTES`). A longer listing is an error that asks for narrower paths, never a partial
   check.
3. Each name is checked with the read policy (secret files, workspace containment).
4. Shortstat, stat and patch run on the scope plus one `:(exclude,literal)<name>` pathspec per denied name. The
   command line stays short, which matters for the Windows 32K limit.
5. The stat lists at most 50 files, plus `... and N more` and the summary line. The patch is read to `max_bytes`
   plus a 4 KiB margin (`DIFF_READ_MARGIN`), redacted, then cut on a character boundary, so a secret cannot survive
   by being cut in half.

Every diff uses `--no-color --no-ext-diff --no-textconv --no-renames --relative`. No configured diff driver runs,
and only the workspace is shown even when it is a subdirectory of the repository. Structured output:
`files_changed`, `insertions`, `deletions`, `truncated`, and `excluded` (the count of policy-denied files, which are
named in the text).

### `run_verification`, `run_tests`, `run_linter`, `run_typechecker`

`run_verification` runs the configured `verification.steps`, or the named subset, in order. `run_tests`,
`run_linter` and `run_typechecker` run the declared steps of one `kind`. Each step:

1. Is classified in the shell that runs it (`cmd /C` on Windows, `sh -c` elsewhere). A step whose verdict is not
   Allow is `blocked` and does not run.
2. Runs in `workspace/<step.cwd>` with its timeout (`timeout_ms`, or `verification.default_timeout_ms`).
3. On timeout, its whole process tree is killed (`taskkill /T /F` on Windows, the process group on Unix).
4. Reports `passed`, `failed`, `timeout` or `blocked`, with the exit code, duration, the last 2 000 bytes of stdout
   and of stderr (`OUTPUT_TAIL_BYTES`), each redacted, and the failures parsed from the output
   (`harness_core::failures`; at most 10 listed in the text, all in the structured result).

A failing step is information, not a tool error (`isError` stays false). With no steps configured, the text
explains how to add them. `run_tests` may append targets to steps that set `targets = true`: at most 20 targets of
at most 300 characters, from a restricted character set, with no leading `-`, no `..`, no absolute or drive path,
and the read policy applied to the part before any `::`.

### `harness_info` and `inspect_config`

`harness_info`: product, version, workspace, profile, each model slot (slot, id, role, endpoint, context; never API
keys), and the verification step names. `inspect_config`: the effective configuration (redacted) and its
validation findings. Both results are redacted like every other.

### `run_command` and adapters

`run_command` runs one program on PATH with plain arguments and never a shell. Every word uses a small character
set (letters, digits and `_./-=:,@+`; at most 300 characters; at most 32 arguments), so the argument list and the
command line the policy classifies are the same thing. The policy then checks the whole command (secret files,
workspace paths, command class), and only an Allow runs. Programs named by a path are refused. The timeout is at
most 10 minutes. Each `adapter_<name>` tool runs its configured program and fixed arguments, plus caller words when
`accepts_args` is set, through the same path (`execute_plain`) and the same checks.

### Read-only analysis tools

| Tool | What it returns |
|---|---|
| `git_history` | Commits that changed a file or directory, newest first, following renames for a file (default 20, at most 100) |
| `git_log_range` | Commits in `from..to` with changed files and diff totals (default 50, at most 200) |
| `parse_test_failure` | Failing tests, compiler errors, type errors and lint findings from command output (at most 1 MiB in) |
| `repo_map` | Busiest directories, languages with parseable source, and key build files of the tracked files |
| `file_summary` | Language, line count, definitions with their lines, and imports of one source file |
| `symbol_search` | Where a function, class, struct, type or module is defined (exact, prefix or contains) |
| `find_references` | Lines that use an identifier, outside comments and strings |
| `dependency_search` | Dependencies declared by `Cargo.toml`, `pyproject.toml`, `requirements*.txt` and `package.json`, and the files that import one |
| `inspect_artifact` | JSON shape, JSON Lines or CSV field statistics, or the tail of a text log |
| `summarize_experiment` | An experiment directory's files, config (secrets redacted) and final metric values |
| `compare_experiments` | Final metric differences and differing config keys between two experiment directories |

The history tools check revisions against a strict character set, pass `--end-of-options`, and do not report
author emails. The code tools read only tracked files (`git ls-files`) that pass the read policy, skip files over
512 KiB and binary files, and parse at most 2 000 files per call.

### Bounded process runner

Git calls and verification steps share `run_child`. Stdin is null, and stdout and stderr are drained on threads
into bounded buffers: stdout keeps the head (git) or the tail (steps), stderr keeps the tail. The child is polled
against a deadline and its tree is killed on expiry. Git calls also get `GIT_TERMINAL_PROMPT=0` and
`GIT_OPTIONAL_LOCKS=0`, and fail after 30 s (`GIT_TIMEOUT`) with "git <cmd> timed out after 30 s".

OpenCode applies its MCP `timeout` to every request. The generated `opencode.json` therefore sets it to the sum of
the verification step timeouts (steps run in turn), never less than the `git_diff` worst case (4 × 30 s), plus
30 s.

## Tests

- `tools/mod.rs` builds temporary git repositories for: each status category
  (`git_status_reports_each_category`); the diff exclusion in every scope
  (`git_diff_excludes_policy_denied_files_in_every_scope`, `git_diff_is_limited_to_a_workspace_inside_the_repository`);
  stat and list caps (`git_diff_caps_the_stat_listing_and_keeps_the_summary`,
  `git_status_caps_structured_lists_and_reports_counts`); the `max_bytes` clamp and bounded read
  (`git_diff_clamps_max_bytes_and_bounds_the_read`, `git_diff_truncation_never_exposes_a_cut_secret`); a git
  timeout (`git_command_past_its_timeout_is_killed_with_a_clear_error`); verification steps passing, failing,
  blocked and timing out (`run_verification_passing_and_failing_steps`, `run_verification_blocks_risky_steps`,
  `verification_steps_are_classified_in_the_shell_that_runs_them`,
  `execute_kills_a_step_that_exceeds_its_timeout`, `a_failed_step_returns_its_parsed_failures`); and
  `default_tools_are_in_documented_order`.
- `protocol.rs`: the handshake and version echo, errors, line cap
  (`oversized_line_gets_a_parse_error_and_serving_continues`), notifications, redaction of results
  (`tool_results_are_redacted_before_they_leave_the_server`) and of unknown tool names
  (`unknown_tool_name_is_redacted_in_the_error`), and caught panics (`panicking_tool_is_contained`).
- `history.rs`, `verify.rs`, `failures.rs`, `code.rs`, `deps.rs`, `artifacts.rs`, `run.rs` and `adapters.rs` have
  their own unit tests; `tools/test_support.rs` holds shared helpers.

## Limits

- The server is single-threaded; one slow tool call holds every other request until its bound expires.
- Code tools are syntactic: `find_references` does not resolve scopes, overloads or imports.
- Through OpenCode on Windows, `git_status`, `git_diff` and `run_verification` were run end to end on 2026-10-22, and
  `symbol_search`, `find_references` and `run_tests` with targets were run end to end in a later run. The other tools are
  covered by unit tests only.

## Related

- Modules: [core-security.md](core-security.md), [core-config.md](core-config.md),
  [cli-host-opencode.md](cli-host-opencode.md), [cli-commands.md](cli-commands.md), [index](README.md)
- Concepts: [architecture.md](../../concepts/architecture.md), [security.md](../../concepts/security.md)
- Decisions: [docs/decisions/](../../decisions/), in particular
  [ADR-0003](../../decisions/0003-tree-sitter-for-code-intelligence.md)
