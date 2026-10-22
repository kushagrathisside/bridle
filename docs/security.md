# Security model

Status: release 1. The policy and checks are in code and covered by tests (see [progress.md](progress.md)).
This is a practical model, not a formal audit. Read the residual risks section.

## What we protect

- Secrets: `.env` files, private keys, credential files, and tokens in command output. They must not reach a model's context or the logs.
- The workspace: the agent may read and write inside the repo, and nowhere else.
- The machine: no unapproved network calls, no destructive or privileged commands.
- Local-first: repository content is not sent to any service. OpenCode's own catalog fetch is a known exception (see troubleshooting).

## Decisions that shape the policy

- **D3: `Ask` is never allowed.** Under `--auto`, OpenCode approves anything not denied, so only an explicit Allow passes. Ask is denied for tool calls and for permission requests.
- **Fail closed.** An unrecognised program is EXECUTE_RISKY, which is denied. An unparseable command is denied. A missing or broken config blocks every call.
- **The model cannot change the policy.** Arguments are inspected, not trusted. The agent cannot write the harness
  and host control files (`local-agent.toml`, `opencode.json`, `.opencode/`, `.local-agent/`, `.git`), and a
  repository's own config cannot loosen the policy (see [configuration.md](configuration.md)). Verification steps come
  only from config, and each is classified before it runs.

## Command classes

Commands are classified by the program name and its arguments (`command_risk`), in this order of severity:
READ_ONLY, WRITE, EXECUTE_SAFE, EXECUTE_RISKY, NETWORK, DESTRUCTIVE, PRIVILEGED. A compound command gets the class of its
most dangerous segment. Commands are lexed in the shell that will run them: PowerShell on Windows (what OpenCode
1.19.3 uses unless `SHELL` says otherwise), `cmd` for verification steps on Windows, POSIX elsewhere. Nested commands (`$(...)`, backticks, `bash -c`, `cmd /c`, `powershell -Command`) are classified too, to a depth of 4.

Default verdicts in the profile: allow READ_ONLY, WRITE and EXECUTE_SAFE; deny PRIVILEGED; deny everything else (D3).

## Secret handling

- **File tools:** `read`, `grep` and `glob` paths are checked against the secret-file rules (`.env`, `.env.*` except
  examples, `*.pem`, `*.key`, `id_rsa`, `id_ed25519`, `.netrc`, `.npmrc`, credentials files). A match is denied.
- **Shell:** every argument word is checked the same way. A glob that could expand to a secret name (`.e*`, `*.pem`) is denied.
- **Output:** `tool.execute.after` redacts known secret shapes and `KEY=value` assignments before the model sees the output.
- **Logs:** the event log and `agent config show` redact the same patterns.
- **Environment:** `shell.env` blanks sensitive variables (names containing KEY, TOKEN, PASSWORD, SECRET, ...) for shell tools.

## Paths

Paths are canonicalised and must stay inside the workspace. Windows device names, alternate data streams and UNC
paths are refused. Null devices (`NUL`, `/dev/null`) are allowed, so redirection to them works.

## Network

- Model endpoints must be loopback (V4). The HTTP client for them ignores the environment proxy.
- `webfetch` and `websearch` are denied in the generated OpenCode config, and the policy denies network-class commands.
- `share` is disabled and `autoupdate` is off in the generated config.

## Residual risks

Fixed since the first list (2026-10-22):
- Verification timeouts now kill the whole process tree (`taskkill /T` on Windows, a process group on Unix), commit `232cb2c`.
- A configured safe prefix only upgrades EXECUTE_RISKY results; it can no longer relabel write or read-only commands, commit `0f7f2c4`.
- MCP output did not reach the after-hook (0 bytes observed in the run of 2026-10-21). The MCP server now redacts every result itself, text and structured content, commit `7864e88`.
- A `read` call with no file path is denied as malformed, commit `1dfd938`.

Fixed on 2026-10-22, after four code reviews (details in [progress.md](progress.md) and
[docs/modules/](modules/README.md)): classifier bypasses (substitutions in quotes, `>()`, `>&file`, `git -c`, exec
flags, `env -S`, programs named by a path, `cmd` quoting); unresolved `~` and `$VAR` paths, `--opt=path`, brace and
bracket globs; protected control files; a repository config that loosened the policy or chose the server binary;
`launch.extra_args` re-binding the server; `git_diff` showing tracked secret files; unbounded MCP output and git
calls; redaction gaps (escaped quotes, `*_KEY`, `-u user:pass`, credential files); `models stop` signalling a
reused PID.

Still open:
1. **Recursive search over the repo.** `grep -r PATTERN .` reads secret files without naming them, so the path checks cannot see it. The values are caught by output redaction (assignments such as `API_KEY=...`, known token shapes, URL passwords, PEM blocks), tested by fixture row 32. A secret in an unrecognised format would still reach the model.
2. **Whole-request timeout.** A very long streamed generation can be cut off by `timeout_ms`. Matters for the deep model (M5).
3. **Heuristics.** Glob matching uses representative secret names, not the real files. A secret with an unusual file name is caught only by output redaction.
4. **Environment secrets.** Secrets in the Windows environment are visible to every process. Keep them out of the environment where possible. If the `shell.env` hook fails, they also stay visible to the agent's shell commands, and only output redaction remains.
5. **Not a sandbox.** Running tests runs code the model wrote, and an allowed `python script.py` can do anything your
   user account can. The policy stops obvious harm and keeps secrets out of the model's context and the logs; it does
   not contain a determined program. Run the agent on repositories you would run yourself, ideally in a worktree.
