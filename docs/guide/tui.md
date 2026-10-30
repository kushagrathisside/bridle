# Control Center (`agent tui`)

A terminal application with two parts: a **chat** with one of your model slots, and a set of **views** that show what Bridle
is doing around it: configuration, slots, tasks, the event log and the policy decisions.

```bash
agent tui
```

It uses the same `--config`, `--workspace` and `LOCAL_AGENT_CONFIG` as every command. It needs a terminal on both stdin and
stdout and exits with an error otherwise. No other command changes.

## Chat

The chat is the home screen. You write what you want; the model answers, and calls tools when it needs to look at or change
the repository. The tools are the ones `agent mcp` offers to OpenCode (git, tests and lint, code navigation, `run_command`,
the declared adapters, and the optional subagent and task-status tools), plus five file tools that OpenCode brings itself:

| Tool | What it does |
|---|---|
| `read_file` | a text file with line numbers, in pages |
| `list_files` | a folder, skipping `.git`, build output and secret files |
| `search_text` | lines that contain a piece of text |
| `write_file` | a new file, or a complete replacement (atomic: the old content or the new, never half) |
| `edit_file` | an exact, unique text replacement (atomic, keeps the file's permissions) |

**The chat is another host, not a privileged one.** For every tool call it runs the code of the OpenCode hook
(`commands::hook::answer_with`): the policy's verdict, the loop rules of the orchestrator, the event-log entry before the
call, and the redaction and task record after it. The tools run through the MCP server's own call path, so verification
results are recorded and outputs are redacted as they are for OpenCode. A blocked call is answered to the model with the rule
that blocked it, shown in the transcript and logged. `ask` counts as blocked. There is no way to approve a blocked call from
the chat. The file tools act on the path the policy resolved, so a symlink out of the workspace, a path with `..`, a secret
file such as `.env`, and the control files (`local-agent.toml`, `.git`, `.opencode`) are refused for the model like for
OpenCode.

With `[workflow] enabled = true` each chat becomes a task of the orchestrator: it appears in the Tasks view, in `agent task`
and in `agent trace`, and the next request carries the task state block.

| Key | Action |
|---|---|
| `Enter` | send |
| `Ctrl+J`, `Alt+Enter` | new line (a pasted line break is also kept as text) |
| `Ctrl+C` | stop the running turn; with text in the input, clear it; otherwise quit |
| `Ctrl+P`, `/model [slot]` | switch the model slot (when no turn is running) |
| `/clear`, `Ctrl+L` | start a new conversation (a new session for the orchestrator) |
| `PgUp`, `PgDn` | scroll the conversation |
| `Up`, `Down` | earlier messages, when the input is empty |
| `Tab` | leave the input for the menu; `F1` opens help from anywhere |

Limits, stated plainly:

- A stop takes effect after the step in progress: a model call that has started, or a tool that is running, is not interrupted.
  Quitting while a turn runs asks it to stop and waits up to 3 seconds for a tool that is mid-call.
- A turn makes at most 30 model calls. The history is shortened to fit the slot's context window: older tool results first, then
  the oldest exchanges, never a call without its result.
- The server of the slot must be running. The chat does not start it; the Models view does (key `s`, with a confirmation).
- A slot that names `api_key_env` reads its key from that environment variable, as `agent health` does.

## Views

Jump with `1`-`9` (from the menu, or from any view that is not the chat input), or move focus to the menu with `Tab`.

| View | Shows | Source |
|---|---|---|
| 1 Chat | the conversation | the model slot |
| 2 Overview | readiness, one line per check, what to do next, recent decisions | all below |
| 3 Tasks | the orchestrator's tasks with status, phase and loop counters; sessions without a task | task journal, event logs |
| 4 Activity | every logged event, newest first, with search and filters | `.local-agent/tool-events.jsonl`, `events.jsonl` |
| 5 Policy | effective security settings, known limits, every allow, ask and deny with its rule and reason | config, event logs |
| 6 Models | slots, file and runtime presence, managed-server state, last health result | config, `.local-agent/servers/*.pid` |
| 7 Diagnostics | `agent doctor` and health findings, each with cause, impact and next step | doctor, health checks |
| 8 Configuration | loaded files and their trust layer, validation, the effective config (redacted) with the layer each value comes from | config loader |
| 9 Network | the destinations `agent egress` lists, local or remote, and whether proxy variables are set | config only |

Readiness is derived, not assumed. `Ready` needs a valid config, the model file and runtime present for every launchable slot,
and a passing health check for every slot; until a health check has run the state is `Not checked`.

The Tasks view reads the task journal without taking a lock or writing anything. The sessions of the event logs are folded into
their task through the session index; a session with no task is listed as a session. `agent trace` and `agent explain` give
the full record of one task.

## Keys outside the chat input

| Key | Action |
|---|---|
| `q`, `Ctrl+C` | quit |
| `?`, `F1` | help |
| `Tab`, `Shift+Tab` | move focus between the menu and the screen |
| `Up` `Down` `PgUp` `PgDn` `Home` `End` | move |
| `Enter` | open the selected item (a task shows its events, an event opens its task, a decision opens in Activity) |
| `Esc` | clear the filter, then return to the menu |
| `/` | search (Activity, Tasks) |
| `v` `t` `c` | Activity: verdict filter, filter to the selected task, clear. Policy: `v` blocked only |
| `r` | re-read local state now |
| `p` | pause or resume auto-refresh |
| `d` `h` `H` | doctor, quick health, full health |
| `s` `x` | Models: start or stop the selected slot's managed server (asks first) |

## What it does and does not do

- **Passive by default.** Opening, refreshing and navigating the views read local files only. No endpoint is contacted and
  nothing is written. The doctor (`d`), the health checks (`h`, `H`) and the chat contact the configured endpoint, and only
  when asked.
- **Auto-refresh.** Every 2 seconds the TUI stats the event logs, the config files, the task journals, the session index and the
  server records, and re-reads only when a size or modification time changed. `p` pauses it. A refresh keeps the selected event,
  task or slot.
- **Read-only configuration.** There is no editor. Values are shown after redaction, with the layer that last set each key
  (`machine`, `repo`, `--config`, `env`, or `default` when no file sets it). The loader records this next to the merge; the
  merge and the trust check do not read it.
- **Start and stop** run the code behind `agent models serve --detach` and `agent models stop`, after a confirmation screen that
  names the slot, file, runtime and endpoint. Only `y` confirms. Start waits up to 5 minutes for health and is refused when the
  config has errors, the slot has no launch section, another GPU slot runs, a managed server already runs, or the endpoint is
  not on this machine. Stop signals a process only when it still runs the recorded server binary; a stale or reused PID is
  cleared without being signalled. A responding endpoint is never taken as proof that Bridle manages it.
- There is no way to change a policy verdict, edit the configuration or run a command outside the policy.
- Event logs are read from the newest 2 MiB of each file, at most 5000 events. Unreadable lines are counted and shown in a
  banner. Text from logs is redacted again and stripped of control characters before display.

## Terminal notes

- Minimum size 60x14. Below 100 columns the menu is a single row that shows every view if the names fit, shorter names if
  not, and otherwise the current view with its neighbours.
- A paste arrives as one event where the terminal supports bracketed paste. The Windows console does not; there, an Enter that
  arrives together with more input is read as a line break inside a paste, not as "send".
- The input counts every character as one column. Wide characters (CJK, some emoji) take two, so the cursor can sit a
  cell off in a line that contains them.
- `NO_COLOR=1` gives a monochrome theme and `LOCAL_AGENT_TUI_ASCII=1` replaces Unicode symbols. Status is always a symbol plus a
  word.
- Verified on Linux (WSL2) in a pseudo-terminal, with the real binary, the real hook path and a scripted OpenAI-compatible
  server. Not yet run on native Windows.
