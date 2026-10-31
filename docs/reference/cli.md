# Command line

The `agent` binary is the command line of Bridle. This page lists its commands and their options. Options that every
command takes are listed first. Run `agent <command> --help` for the same text on your machine.

## Global options

| Option | Meaning |
|---|---|
| `--config <file>` | A configuration file with the highest precedence. The environment variable `LOCAL_AGENT_CONFIG` does the same. |
| `--workspace <dir>` | The repository root. Defaults to the current folder. |
| `-h`, `--help` | Show the help. |
| `-V`, `--version` | Show the version. `agent --version` prints `agent 0.1.0` in this release. |

## Setup

### `agent init`

Wires Bridle into a repository. See [set up a repository](../guide/set-up-a-repository.md).

| Option | Meaning |
|---|---|
| `--profile <name>` | The profile for a new `local-agent.toml`: `local-only` (the default) or `development`. |
| `--force` | Replace the generated files even if you edited them by hand. |
| `--machine` | Write the machine configuration instead of wiring a repository. |
| `--models-dir <dir>` | With `--machine`: the folder that holds the model files. |
| `--runtimes-dir <dir>` | With `--machine`: the folder that holds the inference runtimes. |
| `--opencode-bin <file>` | With `--machine`: the path to the `opencode` executable. |

### `agent config`

| Command | What it does |
|---|---|
| `config show` | Prints the merged configuration as TOML. Values under secret-like keys are redacted. |
| `config validate` | Checks the configuration. Exits with code 0 when there are no errors, and with code 1 when there are. |
| `config paths` | Lists the configuration files loaded and where the machine configuration is. |

## Models

### `agent models`

| Command | What it does |
|---|---|
| `models list` | Lists the configured slots. |
| `models status [slot]` | Shows each slot's server state: `unloaded`, `warming`, `ready` or `error`. |
| `models serve <slot>` | Starts the server for a slot. |
| `models stop <slot>` | Stops a server that was started with `--detach`. |
| `models manifest [slot]` | Records the file hash, the revision, the runtime build and the hardware in `.local-agent/manifests`. |

`models serve` takes `--detach`, which returns once the server answers its health check and leaves it running, and
`--wait-secs <n>`, which defaults to 300. `models manifest` takes `--no-hash`, which skips the file hash.

### `agent health [slot]`

Runs functional checks against each slot's endpoint, or against one slot. Each check prints one line, and the last line
is the result: `result: pass`, `result: warn` or `result: fail`. The command exits with code 1 when the result is `fail`.

| Option | Meaning |
|---|---|
| `--quick` | Only the liveness check, the model id and a one-word completion. |
| `--json` | Print the checks as JSON. |

### `agent doctor`

Reports the host, the configured runtimes and model files, the endpoints, the OpenCode version and the configuration
issues. Warnings are informational. It exits with code 1 only for a missing configured runtime or a configuration error.
Its options are `--json` and the global options.

## Tasks

### `agent task`

| Command | What it does |
|---|---|
| `task start` | Starts a task. Takes `--prompt <text>`, and `--worktree` to run it in its own git worktree. |
| `task status [task]` | Shows the state of a task, or of the latest one. Takes `--json`. |
| `task resume <task>` | Binds the next new session to an existing task. |
| `task reset` | Ends a block and starts the loop counters again. |
| `task repair` | Rebuilds the cached state from the journal. |
| `task prune` | Removes the folders of finished tasks. |

See [run a task](../guide/run-a-task.md) for what each one prints.

### `agent explain [task]`

Prints the recent decisions of a task with the reason for each. `--limit <n>` shows up to 1000 of them.

### `agent trace [task]`

Prints the ordered record of a task, one line per record. `--limit <n>` shows up to 1000 records, and the default is 50.

## Integration

### `agent mcp`

Serves Bridle's tools over MCP on standard input and output. OpenCode starts it through the generated configuration. It
refuses to start while the configuration has errors. See [MCP tools](mcp-tools.md).

### `agent hook <event>`

Used by the OpenCode plugin. It reads one JSON payload on standard input and writes one JSON answer on standard output.
You do not need to run it yourself, but you can use it to see a verdict; see [how decisions are made](../concepts/how-decisions-are-made.md).
Events: `tool-before`, `tool-after`, `permission`, `shell-env`, `event`, `messages-transform` and `system-transform`.

### `agent egress`

Lists every outbound destination the configuration can reach, and whether a proxy is set. It sends nothing.

## The model-side tools

These commands use a model. Each one needs the slot it uses to be running.

### `agent subagent`

Asks the subagent tier a question about a few files, or asks it to condense a text.

| Option | Meaning |
|---|---|
| `--role <explore\|summarise>` | `explore` answers a task about the inputs; `summarise` condenses one text. |
| `--task <text>` | The question or the instruction. At most 500 characters. |
| `--path <file>` | A file to read, relative to the workspace. Repeat for more files. |
| `--text <text>` | A literal text to condense. |

The tier is off by default. See [models and slots](../concepts/models.md).

### `agent eval-decisions <cases>`

Runs the rules and a model on a file of recorded loop cases and reports how they compare. `--slot <slot>` names the model
that decides the stalled cases, and the default is `micro`. `--out <file>` writes the report to a file.

### `agent bench`

Runs a task corpus under one condition, with `bench run`, or summarises result folders, with `bench report`. This is for
evaluating Bridle, not for daily use.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success, including success with warnings. |
| 1 | An error: a failing check, or a configuration with errors. |
