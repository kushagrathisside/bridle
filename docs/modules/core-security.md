# Module: security policy (`harness-core::security`)

Pure logic: no network, no processes. It decides what one tool call may do. The OpenCode adapter calls it on
every tool call ([cli-host-opencode.md](cli-host-opencode.md)), and so does the MCP server for its own tools
([runtime-mcp.md](runtime-mcp.md)).

| File | Purpose |
|---|---|
| `security/mod.rs` | `ToolClass`, ordered from least to most dangerous |
| `security/tool_policy.rs` | `ToolPolicy::evaluate(&ToolRequest) -> PolicyDecision`: one verdict per call |
| `security/path_guard.rs` | `PathGuard::check`: every path stays inside the workspace |
| `security/command_risk.rs` | `assess_command`: classifies shell text into a `ToolClass` |
| `security/redact.rs` | `Redactor`, `is_sensitive_name`, `is_secret_file` |

## Classes and verdicts

`READ_ONLY < WRITE < EXECUTE_SAFE < EXECUTE_RISKY < NETWORK < DESTRUCTIVE < PRIVILEGED`.

| Class | Default verdict | Examples |
|---|---|---|
| READ_ONLY, WRITE, EXECUTE_SAFE | Allow | `git status`, editing a file, `pytest`, `cargo test` |
| EXECUTE_RISKY | `security.risky_commands` (Ask) | an unknown program, `bin/tool`, `git -c ...`, `make` |
| NETWORK | `security.network` (Ask) | `curl`, `git push`, webfetch |
| DESTRUCTIVE | `security.destructive_commands` (Ask) | `rm -rf`, `git reset --hard`, `git push +main` |
| PRIVILEGED | `security.privileged_commands` (Deny) | `sudo`, `systemctl`, `reg add HKLM`, encoded PowerShell |

Ask is blocked by the host adapter (decision D3), so in release 1 only Allow runs.

## Evaluation order (`ToolPolicy::evaluate`)

The first rule that fires decides. Rule names are in the decision and the event log.

| Step | Rule | Denies |
|---|---|---|
| 1 | `path_guard`, `unresolved_path` | a `workdir` or file path outside the workspace, or one using `~`, `$VAR`, `%VAR%` or a substitution |
| 1 | `protected_path` | Edit, Write or Patch on a control file (below) |
| 2 | `secret_file` | Read, List or Search of a secret file; Edit or Write of one is Ask (so blocked) |
| 2 | `malformed` | a Read with no path; a Shell call with no command |
| 3 | `secret_file`, `path_guard`, `unresolved_path` | a shell argument (including `--opt=value` values, brace alternatives, bracket globs) that names or may expand to a secret, or leaves the workspace |
| 3 | `protected_path` | a shell command above READ_ONLY whose argument or working directory is a control file |
| 3 | `command_risk` | the command's class, mapped to its verdict |
| 3 | `unresolved_path` | an otherwise allowed command with an argument resolved only at run time |
| 3 | `tool_kind` | non-shell tools by family: read tools READ_ONLY, edits WRITE, web tools NETWORK |
| 3 | `harness_tool`, `unknown_tool` | `local-agent_*` MCP tools by name; any other tool is EXECUTE_RISKY |

Protected control files (relative to the workspace root, case-insensitive under Windows rules): `local-agent.toml`,
`opencode.json`, `opencode.jsonc`, `.opencode/`, `.local-agent/`, and `.git` at any depth. Every hook process
re-reads `local-agent.toml`, so without this the model could loosen its own policy, or plant a `core.fsmonitor`
command that the next `git status` runs. Reading these files stays allowed.

## Path guard (`path_guard.rs`)

`PathGuard::check(candidate, base)`, in order:

1. Empty or NUL: refused. `~`, `$VAR`, `${...}`, `$(...)`, backticks or `%NAME%`: `Unresolved`.
2. Windows rules (a parameter, so Linux tests run them too): UNC and device paths, reserved names (`CON`, `NUL`,
   `COM1`, ...), alternate data streams (`a.txt:stream`), and trailing dots or spaces are refused.
3. Join relative paths to the base, normalise `.` and `..` lexically, and require containment in a canonical root
   (case-insensitive under Windows rules).
4. Canonicalise the longest existing ancestor (resolving symlinks and junctions) and check containment again,
   so a symlink inside the workspace cannot point out of it.

Bases must be spelled like the canonical root. A Windows 8.3 short path (`C:\Users\KUSHAG~1\...`) is refused
lexically; the policy always passes canonical bases.

## Command classifier (`command_risk.rs`)

**Which grammar.** `ShellKind::host_default()` follows the host. OpenCode 1.19.3 runs commands in `SHELL` when it
names a known shell, otherwise in PowerShell on Windows (`powershell -NoProfile -Command`) and `sh` elsewhere.
The same text means different things in different shells: a single-quoted `&` is literal in PowerShell and splits
commands in `cmd`. Verification steps run under `cmd /C` on Windows, so the MCP server classifies them as `cmd`
(`assess_in`).

**Lexing.** Quotes per grammar: POSIX `'...'` and `"..."` with backslash escapes; PowerShell backtick escapes;
`cmd` has no single quotes. The lexer splits on `;`, `&&`, `||`, `|`, `&` and newlines, and extracts nested
commands as their own segments: `$(...)` (also inside double quotes and under `cmd`), backticks, `<(...)` and
`>(...)`, `bash -c`, `sh -c`, `cmd /c`, `powershell -Command`, `env -S`, `xargs`. Nesting is limited to depth 4,
and unbalanced quotes are EXECUTE_RISKY ("unparseable").

**Classification.** Wrappers (`env`, `time`, `nice`, `nohup`, `timeout`, `xargs`, `VAR=value` prefixes) are
stripped, then the first matching rule wins:

- A program named by a path (`bin/pytest`, `./x`, `C:\tools\y.exe`) is at least EXECUTE_RISKY.
- Flags that run programs or change config raise the class: git `-c`, `--config-env`, `--exec-path`; `go -toolexec`;
  `cargo --config`, `-Z`; `pytest -p`; `rg --pre`; `find -exec`, `-fprint`.
- Output flags (`-o`, `--output`, `git diff --output`) make a read a WRITE, and the target is path-checked.
- Redirection `>` and `>>` is WRITE; `>&word` writes a file unless the word is a digit or `-`.
- `git config` is READ_ONLY only with `--get`, `--get-all` or `--list`; every other form is EXECUTE_RISKY.
- `git push` with `--force`, `+ref`, `:ref`, `--mirror` or `--prune` is DESTRUCTIVE.

A compound command takes the class of its worst segment. `security.safe_commands` prefixes upgrade an EXECUTE_RISKY
result to EXECUTE_SAFE and nothing else, and only the machine config or `--config` may set them
([core-config.md](core-config.md)). `argument_words` returns every argument word of every segment, outer words
of wrapped commands included, for the path checks above.

## Redaction (`redact.rs`)

`Redactor::redact` keeps keys and replaces values with `[REDACTED]`: PEM blocks; `Authorization`, `Cookie`
and `Bearer` values; assignments whose key is sensitive (`*KEY`, `TOKEN`, `PASSWORD`, `SECRET`, `PASSPHRASE`,
`SESSION_ID`, ...), quoted, unquoted or inside JSON strings with escaped quotes; credential flags (`--token x`,
`-u user:pass`); URL passwords (to the last `@`); and well-known token shapes (`sk-...`, `ghp_...`, `AKIA...`,
JWTs, ...). It is idempotent and never panics. If its patterns fail to compile, it returns `[REDACTED]` for
everything. The full rule list is in the module comment of `redact.rs`.

`redact_json` replaces the value of any sensitive key, whatever its type. `is_sensitive_entry(name, value)` is
shared with `shell_env`, so the variables blanked in the shell are the ones redacted in text. A count name
(`MAX_TOKENS`) is sensitive only when its value is not a number.

`is_secret_file`: `.env` and `.env.*` (templates excepted), keys and keystores (`*.pem`, `*.key`, `*.p12`, `*.pfx`,
`*.jks`, `*.keystore`), `.netrc`, `.npmrc`, `.pypirc`, `.git-credentials`, `.pgpass`, `credentials.json`,
`client_secret*.json`, `*.tfvars`, cloud CLI stores (`.aws/credentials`, `.docker/config.json`, `.kube/config`,
`.azure/`, `.config/gcloud/`, `.gnupg/`), and private SSH files.

## Limits (by design, documented in [security.md](../security.md))

- The policy is not a sandbox. Running tests runs code the model wrote, and an allowed `python script.py` can do
  anything the user can. The policy stops obvious harm and keeps secrets out of model context and logs.
- `grep -r PATTERN .` reads secret files without naming them. Their values are caught only by output redaction.
- Glob checks use representative secret names, so a secret with an unusual name is caught only by redaction.

## Tests

- `tool_policy.rs`: every rule above, including protected paths under Windows rules and expansion words.
- `command_risk.rs`: review regression tests (one per finding), shell grammar selection, Windows `cmd` and
  PowerShell cases run on Linux too, and the extra-safe tests.
- `path_guard.rs` and `tests/security_paths.rs`: traversal, symlinks, every Windows device form, ADS, UNC, case.
- `redact.rs`: each rule positive and negative, idempotence, JSON walking, name and file classification.
- `tests/security_commands.rs`: black-box classification cases.
- `crates/harness-cli/tests/hook_fixtures.rs`: 34 OpenCode payloads through the real `agent hook` binary.
