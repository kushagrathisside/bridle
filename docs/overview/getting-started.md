# Getting started

This page takes you from a Windows machine to a working Bridle, in two parts. **Part 1 needs no model:** you install Bridle and watch
it make a policy decision. **Part 2 starts a model** and opens the window on a project.

You need Windows 11 and, for the model, an NVIDIA GPU with a current driver. The full list is in [install Bridle](../guide/installation.md).

## Part 1: install Bridle and see a decision

### 1. Install

From the release zip, unpack it, check the download, and run the installer:

```bat
certutil -hashfile bridle.exe SHA256
install.cmd
```

The first command prints a hash to compare with `SHA256SUMS.txt`. `install.cmd` copies `bridle.exe` to
`%LOCALAPPDATA%\Bridle\bin` and adds that folder to your user `PATH`; it changes nothing else. Open a **new** terminal and check:

```bat
bridle --version
```

```text
bridle 0.1.0
```

To build from source instead, see [install Bridle](../guide/installation.md#build-from-source).

### 2. See a policy decision

Bridle decides each tool call before it runs, and you can watch that without a model. Make an empty folder, save a file `try-read.json`
in it that describes a call the way OpenCode sends it:

```json
{"sessionID":"try-1","callID":"c1","tool":"read","args":{"filePath":"config/.env"}}
```

and run the real hook on it:

```bat
bridle hook tool-before --workspace . < try-read.json
```

The output, on one line:

```json
{"action":"block","message":"blocked by policy (READ_ONLY, secret_file): config/.env may contain credentials and is never sent to a model","class":"READ_ONLY","verdict":"deny","rule":"secret_file"}
```

The call is blocked, the rule that blocked it is named, and the reason is in the message. The file does not need to exist.

### 3. Check the machine

```bat
bridle doctor
```

`doctor` checks the host, the runtimes, the model files, the endpoints and the configuration. Before you have set anything up, it
reports the endpoints as unreachable; those are warnings. It exits with code 1 only when a configured runtime is missing or the
configuration has errors.

## Part 2: a model and a project

### 4. Get the model files

Bridle does not download models. [Serve the models](../guide/models.md) lists the files, where to get them and their SHA-256 values.
You need the `main` model (Qwen3.5-9B) and the llama.cpp runtime; the other slots are optional. Put them in two folders, for example
`D:\models` and `D:\runtimes`.

### 5. Open a project

```bat
cd D:\projects\myproject
bridle
```

The first time on a computer, Bridle asks where your models and runtimes are and writes your machine configuration. The first time in a
project folder, it asks before it writes `bridle.toml`, a `.bridle\` state folder and the `.gitignore` lines for them. It then starts the
default model by itself; the first start takes about ten seconds, and the status bar shows `unloaded`, then `warming`, then `ready`. Type
in the chat and press Enter.

To check a model from the terminal:

```bat
bridle models serve main --detach
bridle health main
bridle models stop main
```

`health` ends with `result: pass` when the server works. If it ends with `result: fail`, read the failing line and
[troubleshoot](../guide/troubleshooting.md). Always stop a model with `bridle models stop`, never with a task killer.

### 6. Or use OpenCode

If you prefer OpenCode's interface, set the repository up for it and run it as usual:

```bat
bridle init --profile local-only
bridle config validate
opencode run "Describe the files in this repository."
```

`init` writes `bridle.toml`, `opencode.json` and `.opencode/plugins/bridle.ts`, and adds `.bridle/` and `.worktrees/` to `.gitignore`.
OpenCode loads the plugin, so every tool call goes through the same policy. A repository with no verification steps gets one warning
from `config validate`, which is expected until you add a test command (see [run your tests](../guide/run-verification.md)).

## What success looks like

- `bridle --version` prints `bridle 0.1.0`.
- The policy check in step 2 returns `"action":"block"` for the `.env` file.
- `bridle doctor` exits with code 0 and lists no errors.
- `bridle health main` ends with `result: pass` once the model is running.
- Every verdict is recorded in `.bridle/tool-events.jsonl` in the project.

## Next steps

- [Run the worked example](../examples/fix-a-failing-test.md): the agent fixes a failing test in a small repository.
- [The desktop app](../guide/desktop.md) for the window, and [team and board](../guide/team-and-board.md) for working with members.
- [Configuration](../guide/configuration.md) for every key and its default.
