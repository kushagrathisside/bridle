# Troubleshooting

This page lists the problems people hit most often on the tested machine, what causes each one, and how to fix it. Each
entry gives the message you will see, so you can search for it.

## The model server is not running

**Message:** `health: endpoint unreachable: ... (os error 10061)`, or `endpoint unreachable` from `agent doctor`.

**Cause:** nothing is listening on the model's address. The server has not been started, or it has stopped.

**Fix:** check the state, then start the slot.

```bat
agent models status
agent models serve main --detach
agent health main
```

`models status` shows `unloaded` when no server is running. `health main` ends with `result: pass` once the server answers.

## A server starts and then exits

**Message:** the server exits during start-up, and `serve` reports an error.

**Cause:** usually a missing model file, or a runtime path that does not match the machine configuration.

**Fix:** read the server's log, which is written beside the project's state:

```text
.local-agent\logs\main.log
```

Check the file name in the configuration with `agent config show`, and check the folder with `agent config paths`.

## `config validate` prints a warning about verification

**Message:** `⚠ [V15] verification.steps - verification is required for DONE but no steps are defined`

**Cause:** the repository has no test command declared.

**Fix:** add a verification step. See [run your tests](run-verification.md). The warning is informational, and the command
still exits with code 0.

## A command is blocked as an unrecognised program

**Message:** `blocked by policy (EXECUTE_RISKY, command_risk): ...` or a message that names an unknown program.

**Cause:** the program is not on Bridle's safe list. Unknown programs are treated as risky, and risky commands are blocked.

**Fix:** if the command is one your project uses for testing or building, add its program name to `security.safe_commands`
in the **machine** configuration. A project file cannot do this. See [run your tests](run-verification.md#run-a-command-that-is-not-built-in).

## The agent does not stop repeating itself

**Cause:** the task workflow is off. The loop rules are part of the workflow, and the default profile leaves it off.

**Fix:** turn the workflow on in the project file, and start a task:

```toml
[workflow]
enabled = true
```

See [run a task](run-a-task.md).

## `config show` prints `secrets_redaction` as `[REDACTED]`

**Cause:** the redaction rule matches the word "secret" in the key name, so the value is hidden in the display.

**Status:** this is a known display problem. The setting is in effect. Bridle's redaction does not depend on how the value is
shown. It is listed in [status and limits](../project/status.md).

## OpenCode starts with a blank window, or stops before the model answers

**Cause:** on the tested machine, OpenCode fetches its model catalogue when it starts, and a failed fetch can leave the
interactive window with nothing drawn. The cause of the blank window is not yet confirmed. A proxy setting can also make
the plugin loader stall.

**What to try:**

1. Clear the proxy variables for the OpenCode process:

   ```bat
   set HTTP_PROXY=
   set HTTPS_PROXY=
   set http_proxy=
   set https_proxy=
   ```

2. Start OpenCode without the interactive window, which runs the same plugin and tools:

   ```bat
   opencode run "Describe the files in this repository."
   ```

3. The benchmark runner starts OpenCode with these switches, which avoid the catalogue fetch and the plugin installs:

   ```text
   OPENCODE_DISABLE_MODELS_FETCH=1
   OPENCODE_DISABLE_AUTOUPDATE=1
   OPENCODE_DISABLE_DEFAULT_PLUGINS=1
   OPENCODE_DISABLE_LSP_DOWNLOAD=1
   NPM_CONFIG_REGISTRY=http://127.0.0.1:9/
   BUN_CONFIG_REGISTRY=http://127.0.0.1:9/
   ```

   With these set, a run got past the catalogue fetch on the tested machine. A complete run with a model using these
   switches is not yet recorded in this release.

## `cargo` cannot download crates

**Cause:** a proxy in the git configuration is picked up by cargo.

**Fix:** vendor the crates from a machine that can reach the registry, with `cargo vendor`, then build offline:

```bat
cargo build -p harness-cli --bin agent --offline --config "source.crates-io.replace-with='vendored-sources'"
```

## A Windows path is blocked

**Message:** `blocked by policy` with a reason that mentions an alternate data stream, or a path outside the workspace.

**Cause:** the path form is one the policy refuses. Commands are split the way the shell that runs them would split them.

**Fix:** write the path the way the shell expects. PowerShell and `cmd` accept `D:\dir`. Git Bash uses POSIX rules, so write the
path as `D:/dir` there.

## The interactive window is not the only way to run Bridle

If OpenCode's interactive window does not work on your machine, the non-interactive commands run the same plugin, the same
policy and the same tools. Use `opencode run` for a single task, or `opencode serve` with `opencode run --attach` for a
server you keep running.
