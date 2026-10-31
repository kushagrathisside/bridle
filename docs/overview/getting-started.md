# Getting started

This page takes you from a Windows machine to a first policy decision, and then to a repository the agent can work in.
It has three parts. The first two do not need a model. The third does.

You need Windows 11, the Visual Studio 2022 C++ build tools, and Git. The full list is in [install Bridle](../guide/installation.md).

## 1. Build the `agent` command

```bat
git clone https://github.com/kushagrathisside/bridle.git
cd bridle
cargo build -p harness-cli --bin agent
```

Cargo uses the Rust version pinned in `rust-toolchain.toml`. The binary is `target\debug\agent.exe`. Check it:

```bat
target\debug\agent.exe --version
```

Expected output:

```text
agent 0.1.0
```

From here on, the examples call the binary as `agent`. Add its folder to your `PATH`, or write out the full path.

## 2. See a policy decision

Bridle decides each tool call before it runs. You can see that without a model. Make a folder with a JSON file that
describes a call, the way OpenCode sends it:

```json
{"sessionID":"try-1","callID":"c1","tool":"read","args":{"filePath":"config/.env"}}
```

Save it as `try-read.json` in an empty folder and run:

```bat
agent hook tool-before --workspace . < try-read.json
```

Expected output, on one line:

```json
{"action":"block","message":"blocked by policy (READ_ONLY, secret_file): config/.env may contain credentials and is never sent to a model","class":"READ_ONLY","verdict":"deny","rule":"secret_file"}
```

The call is blocked, the rule that blocked it is named, and the reason is in the message. The file does not need to exist.

## 3. Set up a repository

Go to the repository you want the agent to work in. Use a clean copy or a branch: the agent will edit files there.

```bat
cd path\to\your\repo
agent init --profile local-only
agent config validate
```

`init` writes four things:

```text
✓ wrote local-agent.toml (profile local-only)
✓ wrote opencode.json
✓ wrote .opencode/plugins/local-agent.ts
✓ .gitignore: added .local-agent/, .worktrees/
```

`config validate` checks the configuration. A repository with no verification steps gets one warning, which is expected
until you add a test command (see [run your tests](../guide/run-verification.md)):

```text
⚠ [V15] verification.steps - verification is required for DONE but no steps are defined
```

The command exits with code 0 when there are only warnings, and with code 1 when there is an error.

## 4. Start a model and check it

Bridle needs a model server for the agent's own reasoning. Download the model files and put them where your
configuration expects them: [serve the models](../guide/models.md) explains the layout and the checksums.

```bat
agent models serve main --detach
agent health main
```

`serve --detach` starts the server and returns when it answers its health endpoint. `health` runs a set of checks, one
line each, and ends with a result line. A fully working server ends with `result: pass`. If it ends with `result: fail`,
read the failing line and [troubleshoot](../guide/troubleshooting.md).

## 5. Run the agent

From the repository folder, start a task through OpenCode:

```bat
opencode run "Describe the files in this repository."
```

OpenCode loads the Bridle plugin from `.opencode/plugins/`, so every tool call goes through the policy. The record is in
`.local-agent/` in the repository.

## What success looks like

- `agent --version` prints `agent 0.1.0`.
- The policy check in step 2 returns `"action":"block"` for the `.env` file.
- `agent config validate` exits with code 0 and lists no errors.
- `agent health main` ends with `result: pass`, once the model is running.
- `opencode run` answers. Every verdict is recorded in `.local-agent/tool-events.jsonl`. `agent explain` shows the
  loop decisions once the task workflow is enabled; see [run a task](../guide/run-a-task.md).

## Next steps

- [Run the worked example](../examples/fix-a-failing-test.md): the agent fixes a failing test in a small repository.
- [Set up a repository](../guide/set-up-a-repository.md) for profiles and the generated files.
- [Configuration reference](../guide/configuration.md) for every key and its default.
