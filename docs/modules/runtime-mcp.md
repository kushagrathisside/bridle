# Module: MCP server and tools (`harness-runtime::mcp`)

`agent mcp` serves deterministic tools to OpenCode over stdio. OpenCode lists them as `local-agent_<tool>`, and
every call still passes the `tool-before` hook first ([cli-host-opencode.md](cli-host-opencode.md)).

| File | Purpose |
|---|---|
| `mcp/protocol.rs` | JSON-RPC 2.0 over newline-delimited stdio; dispatch; redaction of every result |
| `mcp/tools.rs` | `git_status`, `git_diff`, `run_verification`, `harness_info`; the bounded process runner |
| `crates/harness-cli/src/commands/mcp.rs` | Builds the `ToolContext` (workspace, policy, redactor, verification, config) |

## Protocol (`protocol.rs`)

- Methods: `initialize` (echoes a supported protocol version: 2025-06-18, 2025-03-26, 2024-11-05; otherwise the
  newest), `notifications/initialized`, `ping`, `tools/list`, `tools/call`. Anything else is -32601.
- Input is bounded. A line longer than 4 MiB is discarded and answered with a -32700 parse error (id null), and
  the server keeps serving. Invalid UTF-8 or JSON is -32700.
- Notifications get no reply, even when their envelope is invalid. Requests with a bad envelope get -32600.
- Every tool result, text and structured content alike, passes through the redactor before it is written. The
  after-hook does not see MCP output in OpenCode 1.19.3, so this is the only redaction step for these tools.
- An unknown tool name is redacted in its error. A panicking tool is caught (`catch_unwind`) and reported as
  `isError: true` without crashing the server.
- The server is single-threaded, so every tool is bounded in time and output (below).

## Tools (`tools.rs`)

### `git_status`

`git status --porcelain=v2 --branch -z`, parsed by the pure `parse_porcelain_v2` into branch, upstream,
ahead and behind counts, and staged, unstaged, untracked and conflicted paths. Text and structured lists show at
most 50 paths each, with `<list>_count` holding the full count. Renames do not reveal the original path.

### `git_diff`

Arguments: `staged`, `paths`, `stat_only`, `max_bytes` (default 60 000, clamped to 1 000 000).

1. Each requested path must pass the read policy.
2. The changed names in the scope (the paths, or `.`) are listed with `git diff --name-only -z --relative`, read
   up to 4 MiB. A longer listing is an error that asks for narrower paths, never a partial check.
3. Each name is checked with the read policy (secret files, workspace containment).
4. Shortstat, stat and patch run on the scope plus one `:(exclude,literal)<name>` pathspec per denied name. The
   command line stays short, which matters for Windows' 32K limit.
5. The stat lists at most 50 files, plus `... and N more` and the summary line. The patch is read to `max_bytes`
   plus a 4 KiB margin, redacted, then cut on a character boundary, so a secret cannot survive by being cut in half.

Every diff uses `--no-color --no-ext-diff --no-textconv --no-renames --relative`. No configured diff driver
runs, and only the workspace is shown even when it is a subdirectory of the repository. Structured output:
`files_changed`, `insertions`, `deletions`, `truncated`, and `excluded` (the count of policy-denied files, which
are named in the text).

### `run_verification`

Runs the configured `verification.steps`, or the named subset, in order. Each step:

1. Is classified in the shell that runs it (`cmd /C` on Windows, `sh -c` elsewhere). A step whose class is not
   allowed is `blocked` and does not run.
2. Runs in `workspace/<step.cwd>` with its timeout (`timeout_ms`, or `verification.default_timeout_ms`).
3. On timeout, its whole process tree is killed (`taskkill /T /F` on Windows, the process group on Unix).
4. Reports `passed`, `failed`, `timeout` or `blocked`, with the exit code, duration, and the last 2 000 bytes of
   stdout and of stderr, each redacted.

A failing step is information, not a tool error (`isError` stays false). With no steps configured, the text
explains how to add them.

### `harness_info`

Product, version, workspace, profile, each model slot (slot, id, role, endpoint, context; never API keys), and
the verification step names. The whole result is redacted like every other.

## Bounded process runner

Git calls and verification steps share `run_child`. Stdin is null, and stdout and stderr are drained on threads
into bounded buffers: stdout keeps the head (git) or the tail (steps), stderr keeps the tail. The child is polled
against a deadline and its tree is killed on expiry. Git calls also get `GIT_TERMINAL_PROMPT=0` and
`GIT_OPTIONAL_LOCKS=0`, and fail after 30 s with "git <cmd> timed out after 30 s".

OpenCode applies its MCP `timeout` to every request. The generated `opencode.json` therefore sets it to the sum
of the verification step timeouts (steps run in turn), never less than `git_diff`'s worst case (4 × 30 s),
plus 30 s.

## Tests

`tools.rs` builds temporary git repositories for: each status category; the diff exclusion in every scope (no
paths, `.`, a directory, a workspace that is a repository subdirectory); stat and list caps; the `max_bytes`
clamp and bounded read; a git timeout; verification steps passing, failing, blocked (including the
cmd-versus-PowerShell case), and timing out with their tree killed. `protocol.rs` covers the handshake, errors,
line cap, notifications, redaction of results and of unknown tool names, and caught panics.

Not yet run through OpenCode: `git_diff` and `run_verification` (only `git_status` has been, on 2026-10-21).
