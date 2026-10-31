# Security

This page describes what Bridle protects, the rules it applies to each tool call, and what it does not protect. It is for
engineers deciding whether to run the harness on a repository. It describes the design and the tests that cover it. It is
not an independent audit. Read [limits](#limits) before you rely on it.

For the architecture behind these rules, see [architecture](architecture.md).

## What it protects

| Asset | What Bridle does |
|---|---|
| Secrets | Keeps secret files, secret-looking environment variables and secret values in command output out of the model's context and out of the logs. |
| The workspace | Limits file tools and shell commands to the repository and to any folder listed in `security.allowed_paths`. |
| The machine | Denies privileged commands and asks for a decision on network and destructive commands, as set by `security.*`. |
| The harness | Stops the agent from changing the harness configuration, the generated OpenCode files, the state folder, the worktrees folder and `.git`. |
| Repository content | Model traffic goes only to loopback endpoints. OpenCode fetches its own model catalogue at startup, outside Bridle's control. See [troubleshooting](../guide/troubleshooting.md). |

## Rules for every tool call

- **`ask` is a denial.** The permission hook reports `ask` to OpenCode as `deny`, so OpenCode's `--auto` mode cannot approve
  a call the policy did not allow.
- **Failures close.** A program the policy does not recognise is EXECUTE_RISKY, which is denied by default. A command that
  cannot be parsed is denied. If `agent hook` cannot answer, the shim blocks the call. If redaction cannot run, the shim
  withholds the tool output.
- **The agent cannot change the harness.** Writes to these are refused: `local-agent.toml`, `opencode.json`,
  `opencode.jsonc`, `.opencode/`, `.local-agent/`, `.worktrees/` and `.git` at any depth.
- **A repository cannot loosen the policy.** A repository's own configuration may only make the policy stricter. Which
  settings it may set is listed in the [configuration reference](../guide/configuration.md#what-a-repository-file-cannot-set).
- **Verification steps come from configuration.** Each step is classified by the same command rules before it runs.

## Command classes and default verdicts

Each command is classified by its program and arguments. The classes, from least to most severe, are:

`READ_ONLY < WRITE < EXECUTE_SAFE < EXECUTE_RISKY < NETWORK < DESTRUCTIVE < PRIVILEGED`

| Class | Setting | Default verdict | Typical commands |
|---|---|---|---|
| READ_ONLY | none | Allow | Inspecting files and repository state. |
| WRITE | none | Allow | Editing files. |
| EXECUTE_SAFE | none | Allow | Test, lint and build commands. |
| EXECUTE_RISKY | `security.risky_commands` | Ask, so denied | Programs the policy does not recognise, and arbitrary scripts. |
| NETWORK | `security.network` | Ask, so denied | Network access, such as `curl` or pushing to a remote. |
| DESTRUCTIVE | `security.destructive_commands` | Ask, so denied | Deleting or resetting work, such as `rm -rf`. |
| PRIVILEGED | `security.privileged_commands` | Deny | Privilege elevation and system administration. |

Two further rules apply:

- Nested commands, such as `$(...)`, backticks, `bash -c` and `cmd /c`, are classified up to four levels deep. Anything
  nested deeper is EXECUTE_RISKY.
- A prefix listed in `security.safe_commands` applies only to commands that would be EXECUTE_RISKY, and only the machine
  configuration may list one.

## Secrets

| Surface | What happens |
|---|---|
| File tools | `read`, `grep` and `glob` paths are checked against secret file names. The list includes environment files, private keys and certificates, `.netrc`, `.npmrc` and credential files. A match is denied. |
| Shell | Every word of a command is checked the same way, so a glob pattern that could match a secret file name is denied. |
| Tool output | `tool.execute.after` redacts known secret shapes and `KEY=value` assignments before the model sees the output. The MCP server redacts the text it returns. |
| Logs | The event log and `agent config show` apply the same redaction. |
| Environment | The `shell.env` hook sets secret-looking variables to empty values for shell tools. Their names contain words such as `KEY`, `TOKEN`, `PASSWORD` and `SECRET`. |

## Paths

- Paths are resolved and must stay inside the workspace, or inside a folder listed in `security.allowed_paths`.
- Windows device names (such as `CON` and `NUL`), alternate data streams (`file:stream`), and UNC and device-namespace
  paths are refused.

## Network

- Model endpoints must be on this machine. The client for them uses no proxy, so loopback traffic is never sent through one.
- The client for the optional Jev service uses the proxy named in the environment, and it trusts the system certificate
  store.
- Web fetch and web search follow `security.network`, which is `ask` by default, so they are denied unless you change it.
- When the Jev service is enabled, each request for a stalled loop includes the last action, cut to 120 characters. That
  text can contain a command or a path. Leave Jev off unless the text may leave the machine.

## Limits

- **Bridle is not a sandbox.** Verification and the commands the agent runs execute code under your user account. An allowed
  command can do anything your account can do.
- **It does not contain a determined program.** It stops obvious harm and keeps secrets out of the model's context and the
  logs. It does not isolate a process that is set on bypassing it.
- **Recursive search can read secrets without naming them.** A command such as `grep -r PATTERN .` reads secret files without
  any path check firing. Output redaction catches the secret shapes it recognises. A secret in an unrecognised format reaches
  the model.
- **The model can weaken a test.** Nothing in the policy stops an edit to a test file. A test under a protected path, such as
  `.opencode/`, cannot be edited by the agent.
- **Loop rules are heuristics.** A refusal can block a deliberate retry, and the refusal names the rule so you can decide.
  Read-only calls are never refused by a loop rule.
- **Compaction shortens older results.** The model may act on partial output. Compaction does not change any verdict.
- **Worktrees are not enforced.** `security.require_worktree` is read by the configuration checks, but no code path requires
  a worktree yet.

Run the agent on repositories you would run yourself, preferably in a worktree.

## Related

- [Configuration reference](../guide/configuration.md): the security keys and which file may set them.
- [Architecture](architecture.md): the components and the path of a tool call.
- [Module documentation](../reference/modules/core-security.md): the evaluation order, path guard, command classifier and redaction in code.
- [Troubleshooting](../guide/troubleshooting.md): blocked commands and paths.
