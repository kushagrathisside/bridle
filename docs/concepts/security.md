# Security model

This page states what the harness protects, the rules it applies to every tool call, and the risks that remain.
It is for engineers deciding whether to run the harness on a repository. It is a practical model, not a formal
audit, and it assumes the architecture in [architecture.md](architecture.md). Read [Residual risks](#residual-risks)
before you rely on it.

> **Status:** release 1 (native Windows), milestones M0 to M2 done. The policy and checks are in code and covered by
> tests, including 34 hook fixtures through the real `agent hook` binary; review fixes verified on Windows on
> 2026-10-22 (see [progress.md](../status/progress.md)).

## What is protected

| Asset | Rule |
|---|---|
| Secrets | `.env` files, private keys, credential files, and tokens in command output must not reach a model's context or the logs. |
| The workspace | The agent may read and write inside the repository, and nowhere else. |
| The machine | No unapproved network calls, no destructive or privileged commands. |
| Repository content | Not sent to any service (local-first). OpenCode's own catalog fetch is a known exception (see [troubleshooting.md](../guide/troubleshooting.md#opencode-hangs-at-startup-or-debug-config-never-returns)). |

## Decisions that shape the policy

- **D3: Ask is never allowed.** Under `--auto`, OpenCode approves anything not denied, so only an explicit Allow
  passes. Ask is treated as a denial for tool calls and for permission requests. An unanswered `permission` request
  is `deny`.
- **Fail closed.** An unrecognised program is EXECUTE_RISKY, which is denied. An unparseable command (for example,
  unbalanced quotes) is denied. A `read` with no file path is denied as malformed. A missing or broken config blocks
  every call, and if `agent hook` cannot answer, the shim blocks the call.
- **The model cannot change the policy.** Arguments are inspected, not trusted. The agent cannot write the harness
  and host control files (`local-agent.toml`, `opencode.json`, `opencode.jsonc`, `.opencode/`, `.local-agent/`, and
  `.git` at any depth); reading them stays allowed. A repository's own config cannot loosen the policy (see
  [configuration.md](../guide/configuration.md#which-file-may-set-what)). Verification steps come only from config,
  and each is classified before it runs.

## Command classes and verdicts

Commands are classified by the program name and its arguments (`command_risk`). Classes, from least to most severe:

`READ_ONLY < WRITE < EXECUTE_SAFE < EXECUTE_RISKY < NETWORK < DESTRUCTIVE < PRIVILEGED`

| Class | Config key | Default verdict | Result in release 1 | Examples |
|---|---|---|---|---|
| READ_ONLY | | Allow | Runs | `git status` |
| WRITE | | Allow | Runs | editing a file |
| EXECUTE_SAFE | | Allow | Runs | `pytest`, `cargo test` |
| EXECUTE_RISKY | `security.risky_commands` | Ask | Blocked (D3) | an unknown program, `bin/tool`, `git -c ...`, `make` |
| NETWORK | `security.network` | Ask | Blocked (D3) | `curl`, `git push`, webfetch |
| DESTRUCTIVE | `security.destructive_commands` | Ask | Blocked (D3) | `rm -rf`, `git reset --hard`, `git push +main` |
| PRIVILEGED | `security.privileged_commands` | Deny | Blocked | `sudo`, `systemctl`, `reg add HKLM`, encoded PowerShell |

Classification rules:

| Rule | Detail |
|---|---|
| Compound commands | A compound command gets the class of its most dangerous segment. |
| Shell grammar | Commands are lexed in the shell that will run them: PowerShell on Windows (what OpenCode 1.19.3 uses unless `SHELL` says otherwise), `cmd` for verification steps on Windows, POSIX elsewhere. |
| Nested commands | `$(...)`, backticks, `bash -c`, `cmd /c`, `powershell -Command` (and the other forms in [core-security.md](../reference/modules/core-security.md#command-classifier-command_riskrs)) are classified too, to a depth of 4. |
| Safe prefixes | A configured `security.safe_commands` prefix only upgrades an EXECUTE_RISKY result to EXECUTE_SAFE. Only the machine config or `--config` may set one. |

## Secret handling

| Surface | Mechanism |
|---|---|
| File tools | `read`, `grep` and `glob` paths are checked against the secret-file rules (`.env`, `.env.*` except examples, `*.pem`, `*.key`, `id_rsa`, `id_ed25519`, `.netrc`, `.npmrc`, credentials files; the full list is in [core-security.md](../reference/modules/core-security.md#redaction-redactrs)). A match is denied. |
| Shell | Every argument word is checked the same way. A glob that could expand to a secret name (`.e*`, `*.pem`) is denied. |
| Output | `tool.execute.after` redacts known secret shapes and `KEY=value` assignments before the model sees the output. The MCP server redacts every result itself, text and structured content. |
| Logs | The event log and `agent config show` redact the same patterns. |
| Environment | `shell.env` blanks sensitive variables (names containing KEY, TOKEN, PASSWORD, SECRET, ...) for shell tools. |

## Paths

- Paths are canonicalised and must stay inside the workspace.
- Windows device names, alternate data streams and UNC paths are refused.
- Null devices (`NUL`, `/dev/null`) are allowed, so redirection to them works.

## Network

- Model endpoints must be loopback (V4). The HTTP client for them ignores the environment proxy.
- `webfetch` and `websearch` are denied in the generated OpenCode config, and the policy denies network-class
  commands.
- `share` is disabled and `autoupdate` is off in the generated config.

## Fixed issues

Fixed since the first list (2026-10-22):

| Issue | Fix | Commit |
|---|---|---|
| Verification timeouts left grandchild processes running | Timeouts kill the whole process tree (`taskkill /T` on Windows, a process group on Unix) | `7e5dab3` |
| A configured safe prefix could relabel write or read-only commands | It only upgrades EXECUTE_RISKY results | `6291138` |
| MCP output did not reach the after-hook (0 bytes observed in the run of 2026-10-21) | The MCP server redacts every result itself, text and structured content | `9df0aa4` |
| A `read` call with no file path was allowed | Denied as malformed | `b45b851` |

Fixed on 2026-10-22, after four code reviews (details in [progress.md](../status/progress.md) and the
[module documentation](../reference/modules/README.md)):

- Classifier bypasses: substitutions in quotes, `>()`, `>&file`, `git -c`, exec flags, `env -S`, programs named by a
  path, `cmd` quoting.
- Paths: unresolved `~` and `$VAR` paths, `--opt=path`, brace and bracket globs.
- Protected control files.
- A repository config that loosened the policy or chose the server binary.
- `launch.extra_args` re-binding the server.
- `git_diff` showing tracked secret files.
- Unbounded MCP output and git calls.
- Redaction gaps: escaped quotes, `*_KEY`, `-u user:pass`, credential files.
- `models stop` signalling a reused PID.

## Limits

- The policy is not a sandbox. Running tests runs code the model wrote, and an allowed `python script.py` can do
  anything your user account can.
- The policy stops obvious harm and keeps secrets out of the model's context and the logs. It does not contain a
  determined program.
- Run the agent on repositories you would run yourself, ideally in a worktree.

## Residual risks

1. **Recursive search over the repo.** `grep -r PATTERN .` reads secret files without naming them, so the path checks
   cannot see it. The values are caught by output redaction (assignments such as `API_KEY=...`, known token shapes,
   URL passwords, PEM blocks), tested by fixture row 32. A secret in an unrecognised format would still reach the
   model.
2. **Whole-request timeout.** A very long streamed generation can be cut off by `timeout_ms`. This matters for the
   deep model (slot `deep`, M5).
3. **Heuristics.** Glob matching uses representative secret names, not the real files. A secret with an unusual file
   name is caught only by output redaction.
4. **Environment secrets.** Secrets in the Windows environment are visible to every process. Keep them out of the
   environment where possible. If the `shell.env` hook fails, they also stay visible to the agent's shell commands,
   and only output redaction remains.
5. **Not a sandbox.** See [Limits](#limits).

6. **The model can weaken a test to make it pass.** Live run 1 (2026-10-26) removed a contradictory assertion from a
   plain test file. Nothing in the policy prevents an edit of a test file. A test kept under a protected path such as
   `.opencode/` cannot be edited by the agent.
7. **OpenCode stops identical consecutive calls first.** Its `doom_loop` permission ended a live run on the third identical
   `run_tests` call, before the harness could refuse it. The harness's loop rules cover calls that differ, such as an
   edit and re-test cycle. Identical-call loops are handled by the host.
8. **Loop rules are heuristics.** A refusal can block a deliberate retry; the refusal names the rule so the person can
    decide. Read-only calls are never refused by a loop rule, so a model in a loop can still read files.
9. **Refusals are not notices to the model when the cap is reached.** After the refusal cap the harness stops refusing
    on loop grounds and records the notice in `agent trace` and `agent explain`. The model does not see it in a tool
    result.
10. **Compaction hides parts of a result.** A compacted result keeps its head and tail, and a dropped one becomes a stub.
   The model may act on a partial output. Compaction never changes a verdict, and secrets are redacted before a result is
   stored, so it cannot expose one.
11. **Jev sees the loop summary, when it is enabled.** For a stalled loop, Jev receives the phase, the counters, the last
    action (capped at 120 characters) and the names of the loop signals. The last action can contain a command fragment
    or a path. Leave Jev off unless the last action text may leave the machine. The key is read from the environment,
    is sent only as a bearer header, and is not written to the event log or to an error. A remote request goes through
    the proxy the environment names.

## Related

- [architecture.md](architecture.md): components and the path of a tool call.
- [core-security.md](../reference/modules/core-security.md): evaluation order, path guard, classifier and redaction in code.
- [cli-host-opencode.md](../reference/modules/cli-host-opencode.md): hook protocol, tool mapping and generated `opencode.json`.
- [configuration.md](../guide/configuration.md): security keys and which file may set what.
- [troubleshooting.md](../guide/troubleshooting.md): blocked commands, paths and proxy issues.
- [progress.md](../status/progress.md): verification evidence and review findings.
