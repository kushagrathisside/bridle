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
- **The model cannot change the policy.** Arguments are inspected, not trusted. Verification steps come only from the project config.

## Command classes

Commands are classified by the program name and its arguments (`command_risk`), in this order of severity:
READ_ONLY, WRITE, EXECUTE_SAFE, EXECUTE_RISKY, NETWORK, DESTRUCTIVE, PRIVILEGED. A compound command gets the class of its
most dangerous segment. Nested commands (`$(...)`, backticks, `bash -c`, `cmd /c`, `powershell -Command`) are classified too, to a depth of 4.

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

## Residual risks (known, not yet fixed)

1. **Recursive search over the repo.** `grep -r PATTERN .` reads secret files through the search, and the file names don't appear in the command. Output redaction is the backstop, and it has not been tested for this case.
2. **Timeouts and grandchildren.** A verification step that times out has its shell killed, but processes it started can keep running.
3. **`extra_safe` ordering.** A configured safe prefix is checked before the built-in WRITE and READ_ONLY rules, so a prefix such as `cp` could downgrade a WRITE command. Needs a policy review.
4. **Whole-request timeout.** A very long streamed generation can be cut off by `timeout_ms`.
5. **Heuristics.** Glob matching uses representative secret names, not the real files. A secret with an unusual name is caught only by the file-name rules.
6. **Machine config and proxy credentials.** Secrets in the Windows environment are visible to every process. Keep them out of the environment where possible.
