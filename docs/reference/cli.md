# Command line

The `bridle` binary is the command line of Bridle. This page lists its commands and their options. Options that every
command takes are listed first. Run `bridle <command> --help` for the same text on your machine.

## Global options

| Option | Meaning |
|---|---|
| `--config <file>` | A configuration file with the highest precedence. The environment variable `BRIDLE_CONFIG` does the same. |
| `--workspace <dir>` | The repository root. Defaults to the current folder. |
| `-h`, `--help` | Show the help. |
| `-V`, `--version` | Show the version. `bridle --version` prints `bridle 0.1.0` in this release. |

## Setup

### `bridle init`

Wires Bridle into a repository. See [set up a repository](../guide/set-up-a-repository.md).

| Option | Meaning |
|---|---|
| `--profile <name>` | The profile for a new `bridle.toml`: `local-only` (the default) or `development`. |
| `--force` | Replace the generated files even if you edited them by hand. |
| `--machine` | Write the machine configuration instead of wiring a repository. |
| `--models-dir <dir>` | With `--machine`: the folder that holds the model files. |
| `--runtimes-dir <dir>` | With `--machine`: the folder that holds the inference runtimes. |
| `--opencode-bin <file>` | With `--machine`: the path to the `opencode` executable. |

### `bridle config`

| Command | What it does |
|---|---|
| `config show` | Prints the merged configuration as TOML. Values under secret-like keys are redacted. |
| `config validate` | Checks the configuration. Exits with code 0 when there are no errors, and with code 1 when there are. |
| `config paths` | Lists the configuration files loaded and where the machine configuration is. |

## Models

### `bridle models`

| Command | What it does |
|---|---|
| `models list` | Lists the configured slots. |
| `models status [slot]` | Shows each slot's server state: `unloaded`, `warming`, `ready` or `error`. |
| `models serve <slot>` | Starts the server for a slot. |
| `models stop <slot>` | Stops a server that was started with `--detach`. |
| `models manifest [slot]` | Records the file hash, the revision, the runtime build and the hardware in `.bridle/manifests`. |

`models serve` takes `--detach`, which returns once the server answers its health check and leaves it running, and
`--wait-secs <n>`, which defaults to 300. `models manifest` takes `--no-hash`, which skips the file hash.

### `bridle health [slot]`

Runs functional checks against each slot's endpoint, or against one slot. Each check prints one line, and the last line
is the result: `result: pass`, `result: warn` or `result: fail`. The command exits with code 1 when the result is `fail`.

| Option | Meaning |
|---|---|
| `--quick` | Only the liveness check, the model id and a one-word completion. |
| `--json` | Print the checks as JSON. |

### `bridle doctor`

Reports the host, the configured runtimes and model files, the endpoints, the OpenCode version and the configuration
issues. Warnings are informational. It exits with code 1 only for a missing configured runtime or a configuration error.
Its options are `--json`, `--repair` and the global options. `--repair` first puts right what a crash left behind (ended leases, runs cut short, `bridle-index-*` files, unfinished downloads, old temporary files, browser session folders of processes that are gone), says what it did, and does nothing to a run that is alive; see [Runs, checkpoints and recovery](run-recovery.md).

## Tasks

### `bridle task`

| Command | What it does |
|---|---|
| `task start` | Starts a task. Takes `--prompt <text>`, and `--worktree` to run it in its own git worktree. |
| `task status [task]` | Shows the state of a task, or of the latest one. Takes `--json`. |
| `task resume <task>` | Binds the next new session to an existing task. |
| `task reset` | Ends a block and starts the loop counters again. |
| `task repair` | Rebuilds the cached state from the journal. |
| `task prune` | Removes the folders of finished tasks. |

See [run a task](../guide/run-a-task.md) for what each one prints.

### `bridle explain [task]`

Prints the recent decisions of a task with the reason for each. `--limit <n>` shows up to 1000 of them.

### `bridle trace [task]`

Prints the ordered record of a task, one line per record. `--limit <n>` shows up to 1000 records, and the default is 50.

## Integration

### `bridle mcp`

Serves Bridle's tools over MCP on standard input and output. OpenCode starts it through the generated configuration. It
refuses to start while the configuration has errors. See [MCP tools](mcp-tools.md).

### `bridle hook <event>`

Used by the OpenCode plugin. It reads one JSON payload on standard input and writes one JSON answer on standard output.
You do not need to run it yourself, but you can use it to see a verdict; see [how decisions are made](../concepts/how-decisions-are-made.md).
Events: `tool-before`, `tool-after`, `permission`, `shell-env`, `event`, `messages-transform` and `system-transform`.

### `bridle egress`

Lists every outbound destination the configuration can reach, and whether a proxy is set. It sends nothing. With web research on it lists
the search providers and "any public web address"; with the browser on it adds the managed browser and says whether its egress proxy pins
the addresses it connects to or hands names to your proxy.

### `bridle web`

Web research from the terminal. You are the one asking, so these do not need `security.network = "allow"`; they need `[web] enabled`.
See [Web research](../guide/web-research.md).

| Command | What it does |
|---|---|
| `bridle web status` / `providers` | The settings, the route, the recorded sources; each provider and whether its key variable is set. |
| `bridle web search "query" [--news] [--limit N] [--domain D] [--after DATE]` | Search every usable provider and print the fused results. |
| `bridle web open URL` / `download URL [--sha256 HEX]` / `inspect S-0002 or path` | Read a page, save a file, show a saved file. |

### `bridle web browser`

The managed Chromium. See [The managed browser](../guide/web-browser.md) and
[ADR-0020](../decisions/0020-managed-browser-is-a-policy-gated-playwright-worker.md).

| Command | What it does |
|---|---|
| `status` | Enabled or not; the runtime state, versions and the SHA-256 of `chrome.exe`; the limits; how the browser reaches the web. Exit code 1 if the runtime is not ready. |
| `setup` | Downloads the pinned Node.js, playwright-core and Chromium (about 210 MB) through your proxy into `browser-runtime\` next to the machine configuration. Checks each against its published hash. Safe to repeat. |
| `test [--live URL]` | Runs a real Chromium against a small site served from this machine (about 70 checks, no internet). `--live` also reads one real page. Exit code 1 if any check fails. |
| `remove` | Deletes the runtime folder (only a folder named `browser-runtime`). |

`bridle doctor` also prints a `browser` line when the browser is enabled or installed: off, not installed, incomplete, the wrong version,
or ready.

### `bridle lanes`

Separate working copies for parallel members, and the person's merge of them. Needs a real Git repository. See [Lanes](../guide/lanes.md)
and [ADR-0023](../decisions/0023-notices-background-commands-and-lanes-from-infinity.md).

| Command | What it does |
|---|---|
| `new NAME [--from LANE]` | Snapshots the project (uncommitted changes included) as a worktree in `.bridle\lanes\NAME\`; with `--from`, starts from another lane's work. Prints the folder to use as a member's `--workspace`. |
| `list` / `path NAME` | The lanes / a lane's folder. |
| `diff NAME [--stat]` | What the lane changed since it started: files, summary and (without `--stat`) the patch. |
| `integrate NAME... [--dry-run] [--skip-conflicts] [--remove]` | Merges the lanes into the project in the order given, three ways. Nothing is written if one conflicts (unless `--skip-conflicts`); files you changed since the snapshot are left alone and listed. Exit code 1 on a conflict or a file left alone. |
| `undo` | Puts the project back as it was before the last integration. |
| `remove NAME` | Deletes a lane: folder, record and checkpoints. |

### `bridle eval-agent`

Runs a **real model** through the chat loop on scripted scenarios and scores what it did. For evaluating Bridle, not for daily use. The model
server must already be running (`bridle models serve main --detach`). Every scenario runs in a fresh temporary Git repository.

| Command | What it does |
|---|---|
| `notices [--runs N] [--slot S] [--max-seconds N] [--out FILE]` | Does the model start a background command, read the notice that says it ended, and re-read a file after a notice that someone else changed it? Includes a control with no change. |
| `plan [--runs N] [--slot S] [--out FILE]` | The project manager's plan for a request to split: two or more child issues, an engineering and a QA issue, and the QA issue waiting for the engineering one. |
| `lanes [--slot S] [--out FILE]` | Two turns at the same time, each in its own lane, then `lanes integrate`. |

### `bridle desktop`

| Command | What it does |
|---|---|
| `screenshot PROGRAM [ARGS...] [--wait-ms N] [--title TEXT]` | Starts the program (a bare name on `PATH`, judged like `run_command`), waits for its window, saves a picture of that window under `.bridle\web\screenshots\`, stops the program. Needs `[desktop] screenshots_enabled`. See [Window screenshots](../guide/window-screenshots.md) and [ADR-0021](../decisions/0021-window-screenshots-start-the-program-and-capture-only-its-window.md). |

### `bridle fleet`

The small-model fleet and its scheduler; needs `[fleet] enabled`. See [The fleet](../guide/fleet.md) and
[ADR-0022](../decisions/0022-the-fleet-is-small-models-loaded-together-and-a-deterministic-scheduler-swaps-them.md).

| Command | What it does |
|---|---|
| `status` | The slots, whether each runs, what each needs in video memory, whether they fit together, and any unfinished swap. |
| `up [--wait-secs N]` | Checks the fleet fits, writes `.bridle\swap.json`, stops the GPU slots that are not in the fleet, starts the fleet. Refused while a swap is unfinished. |
| `down [--wait-secs N]` / `recover [--wait-secs N]` | Stops the fleet, starts again what `up` stopped, removes the record. `recover` is the same restore, for a swap a crash cut short. |
| `run TASKS.json [--parallel N] [--out FILE] [--keep-up] [--wait-secs N]` | Brings the fleet up, sends the tasks to the worker several at a time, prints each checked answer and the speedup, puts the big model back (unless `--keep-up`). |

## The model-side tools

These commands use a model. Each one needs the slot it uses to be running.

### `bridle subagent`

Asks the subagent tier a question about a few files, or asks it to condense a text.

| Option | Meaning |
|---|---|
| `--role <explore\|summarise>` | `explore` answers a task about the inputs; `summarise` condenses one text. |
| `--task <text>` | The question or the instruction. At most 500 characters. |
| `--path <file>` | A file to read, relative to the workspace. Repeat for more files. |
| `--text <text>` | A literal text to condense. |

The tier is off by default. See [models and slots](../concepts/models.md).

### `bridle eval-decisions <cases>`

Runs the rules and a model on a file of recorded loop cases and reports how they compare. `--slot <slot>` names the model
that decides the stalled cases, and the default is `micro`. `--out <file>` writes the report to a file.

### `bridle bench`

Runs a task corpus under one condition, with `bench run`, or summarises result folders, with `bench report`. This is for
evaluating Bridle, not for daily use.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | Success, including success with warnings. |
| 1 | An error: a failing check, or a configuration with errors. |
