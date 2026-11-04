# Adding and updating models, runtimes and plugins

This page explains how a model, an inference runtime or a plugin is added to Bridle, how it is updated or removed, and when a change takes
effect. All three are declared in one place: the **machine configuration file**. The window shows what has been declared, and its
**Models** and **Info** panes are the quickest way to check that a change took hold, but it does not yet offer a form for editing them.
Declarations are made by editing the file in a text editor.

## Where the file is

The machine configuration is a TOML file, normally `%APPDATA%\bridle\config.toml`. To find the exact path on your computer:

```bat
bridle config paths
```

In the window, choose **Info** in the left-hand bar. The **Machine settings** card shows the path, the models folder and the runtimes
folder, and its **Show in Explorer** button opens Windows Explorer with the file selected. The first time Bridle runs on a computer it
asks for the two folders and writes the file; to write it from the terminal instead:

```bat
bridle init --machine --models-dir D:\models --runtimes-dir D:\runtimes
```

Only the machine configuration may declare models, runtimes and plugins. A repository's own `bridle.toml` cannot, so that a project you
clone cannot choose which programs Bridle starts. The full list of keys is in the [configuration reference](configuration.md).

## When a change takes effect

| What you changed | When it is seen |
|---|---|
| The configuration file | The window reads it again every few seconds, so a new or changed entry appears in the **Models** and **Info** panes by itself. The **Reload** button in either pane reads it at once. Every terminal command reads the file each time it runs. |
| The settings of a model server that is already running | Not until you stop it and start it again (`bridle models stop NAME`, then `bridle models serve NAME --detach`, or **Stop** and **Start** in the **Models** pane). A running server keeps the settings it was started with. |
| A plugin | When a conversation starts. Choose **New chat** in the window; for OpenCode, start a new session. |
| The files themselves (a new model file, a new runtime build) | At the next start of the server. Bridle checks that each file exists whenever it lists the slots and again before it starts a server. |

## Adding a runtime

A runtime is the program that serves a model: `llama-server.exe` from [llama.cpp](https://github.com/ggml-org/llama.cpp).

1. Unpack the build under your runtimes folder, for example `D:\runtimes\llama.cpp\b11756-cuda12.4\`, together with the CUDA runtime
   archive of the same release.
2. Add a section to the machine configuration. A relative `server_bin` is read from `paths.runtimes_dir`:

   ```toml
   [paths]
   runtimes_dir = 'D:\runtimes'

   [runtimes.llama_cpp]
   kind = "llama_cpp"
   server_bin = 'llama.cpp\b11756-cuda12.4\llama-server.exe'
   build = "b11756"
   ```

   Use `kind = "prism_llama_cpp"` for the PrismML fork, which the ternary model formats need.
3. Check it. The **Runtimes** card in the **Info** pane lists each runtime, its file and whether the file is present; or run
   `bridle doctor`, which names the runtime and its build.

## Adding a model

A model is a *slot*: a name, an address on this machine, and, when Bridle should start the server itself, a `launch` section.

1. Download the model file and check its SHA-256 value. The [models guide](models.md) lists the files Bridle has been tested with. Put the
   file under your models folder, for example `D:\models\Qwen3.5-9B\Qwen3.5-9B-Q4_K_M.gguf`.
2. Add the slot to the machine configuration:

   ```toml
   [paths]
   models_dir = 'D:\models'

   [models.main]
   provider = "llama_cpp"
   id = "qwen3.5-9b"
   display_name = "Qwen3.5-9B Q4_K_M"
   role = "engineer"
   endpoint = "http://127.0.0.1:8001/v1"
   hardware = "gpu"
   context = 65536
   max_output_tokens = 4096
   kv_cache = "q8_0"

   [models.main.launch]
   runtime = "llama_cpp"                           # a name under [runtimes]
   model_file = 'Qwen3.5-9B\Qwen3.5-9B-Q4_K_M.gguf' # relative to models_dir

   [models.main.source]
   sha256 = "03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8"
   ```

   The endpoint must be on this machine (`127.0.0.1`, `localhost` or `::1`), and each running slot needs its own port. A model that reads
   pictures also needs `capabilities.vision = true` and `launch.mmproj_file`. To make a slot the one the chat uses by default, set
   `agent.default_model` to its name.
3. Validate, then start and check it:

   ```bat
   bridle config validate
   bridle models serve main --detach
   bridle health main
   ```

   `config validate` names the rule and the key for anything wrong. `health` ends with `result: pass` when the server works. In the window,
   the **Models** pane shows the slot with its runtime and model file, each marked present or missing, and offers **Start**, **Stop** and
   **Health check**.
4. Record what you ran: `bridle models manifest main` writes the file's hash, the runtime build and the hardware, and checks the file
   against `source.sha256`.

Bridle runs **one GPU model at a time** and refuses to start a second one, or one that its own estimate says will not fit in video memory,
leaving at least 768 MiB free. To run several small models together, use the [fleet](fleet.md).

## Updating and removing

- **A newer model file or runtime build.** Put the new file beside the old one, change `model_file` or `server_bin` (and `build`,
  `source.revision` and `source.sha256`) to match, stop the server and start it again. Keep the old file until `bridle health` passes.
- **Removing a model or a runtime.** Stop its server first (`bridle models stop NAME`), then delete its section. A runtime that a slot
  still names is reported by `config validate` (rule V6).
- **A problem.** `bridle doctor` checks the runtimes, the model files and the endpoints and says what to do next; the **Run checks**
  button in the **Models** pane shows the same report. See [troubleshooting](troubleshooting.md).

## Adding a plugin

A plugin is a manifest file that declares tools. Each tool runs one fixed program with fixed arguments and takes **no arguments from the
model** ([ADR-0013](../decisions/0013-plugins-are-declared-and-pass-the-policy.md)). Plugins are listed in the machine configuration only.

1. Write the manifest, for example `D:\plugins\git-tools\plugin.toml`:

   ```toml
   name = "git-tools"
   version = "0.1.0"

   [[tools]]
   name = "short_status"
   description = "List changed files, one per line"
   program = "git"
   args = ["status", "--short"]
   declared = "READ_ONLY"        # or EXECUTE_SAFE
   ```

2. List it by absolute path in the machine configuration (single quotes keep the backslashes literal):

   ```toml
   [plugins]
   manifests = ['D:\plugins\git-tools\plugin.toml']
   ```

3. Check it: the **Plugins** card in the **Info** pane lists each tool with its class and says why any tool was not loaded; the terminal
   equivalent is `bridle plugins`. Then start a new chat. The tool is offered as `plugin_git-tools_short_status`.

The class a tool is offered under is the stricter of the one the manifest declares and the one the command classifier gives its command. A
tool whose command the classifier calls a write, a network use, a destructive command or an interpreter is reported and not offered. A
plugin tool runs through the same checks as `run_command`, in the sandbox.

For a tool that must take a project-specific command, the machine configuration also accepts declarative **adapters**
(`[[tools.adapters]]`; see the [configuration reference](configuration.md)). A repository cannot add either.

## What the window cannot do yet

- It has no form for adding, editing or removing a model, a runtime or a plugin; the file is edited by hand.
- It shows what is declared and whether each file exists, and starts, stops and checks a model server, but it does not download model files
  or runtimes. Bridle never downloads a model on its own.
- It has no "check for updates": newer models and runtimes are chosen and downloaded by you.

## Related

- [Serve the models](models.md) · [Configuration reference](configuration.md) · [The desktop app](desktop.md) · [Troubleshooting](troubleshooting.md)
