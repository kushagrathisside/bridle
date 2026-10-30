# Control Center (`agent tui`)

An interactive terminal view of Bridle: configuration, model slots, the event logs and the policy decisions. It inspects
Bridle. Coding sessions still run in OpenCode; the TUI does not start, drive or observe them.

```bash
agent tui
```

It uses the same `--config`, `--workspace` and `LOCAL_AGENT_CONFIG` as every command. It needs a terminal on both stdin
and stdout and exits with an error otherwise. No other command changes.

## Views

Jump with `1`-`8`, or move focus to the navigation list with `Tab`.

| View | Shows | Source |
|---|---|---|
| 1 Overview | readiness, one line per config, slot and diagnostic check, recent decisions | all below |
| 2 Tasks | tasks or sessions found in the event logs, with allowed, blocked and failed counts | event logs |
| 3 Activity | every logged event, newest first, with search and filters | `.local-agent/tool-events.jsonl`, `events.jsonl` |
| 4 Policy | effective security settings, known limits, every allow, ask and deny with its rule and reason | config, event logs |
| 5 Models | slots, model file and runtime presence, managed-server state, last health result | config, `.local-agent/servers/*.pid` |
| 6 Diagnostics | `agent doctor` and health findings, each with cause, impact and next step | doctor, health checks |
| 7 Configuration | loaded files and their trust layer, validation, the effective config (redacted) with the layer each value comes from | config loader |
| 8 Network | configured destinations, loopback or remote, proxy variables that are set | config only |

Readiness is derived, not assumed. `Ready` needs a valid config, the model file and runtime present for every launchable
slot, and a passing health check for every slot; until a health check has run the state is `Unverified`.

The Tasks view groups logged events by `task_id`, falling back to `session_id`. It does not read the task journal yet:
use `agent task`, `agent explain` and `agent trace` for task state. The Network view reads the configuration only; use
`agent egress` for the full outbound list.

## Keys

| Key | Action |
|---|---|
| `q`, `Ctrl+C` | quit |
| `?` | help |
| `Tab`, `Shift+Tab` | move focus between navigation and content |
| `Up` `Down` `PgUp` `PgDn` `Home` `End` | move |
| `Enter` | open the selected item (a task shows its events, an event opens its task, a decision opens in Activity) |
| `Esc` | clear the filter, then return to navigation |
| `/` | search (Activity, Tasks) |
| `v` `t` `c` | Activity: verdict filter, filter to the selected task, clear. Policy: `v` blocked only |
| `r` | re-read local state now |
| `p` | pause or resume auto-refresh |
| `d` `h` `H` | doctor, quick health, full health |
| `s` `x` | Models: start or stop the selected slot's managed server (asks first) |

## What it does and does not do

- **Passive by default.** Opening, refreshing and navigating read local files only. No endpoint is contacted and nothing
  is written. The doctor (`d`) and the health checks (`h`, `H`) contact the configured endpoints, and only when asked.
- **Auto-refresh.** Every 2 seconds the TUI stats the event logs, the config files and the server records, and re-reads
  only when a size or modification time changed. `p` pauses it. A refresh keeps the selected event, task or slot.
- **Read-only configuration.** There is no editor. Values are shown after redaction, with the layer that last set each key
  (`machine`, `repo`, `--config`, `env`, or `default` when no file sets it). The loader records this next to the merge; the
  merge and the trust check do not read it.
- **Start and stop** run the code behind `agent models serve --detach` and `agent models stop`, after a confirmation
  screen that names the slot, file, runtime and endpoint. Only `y` confirms. Start waits up to 5 minutes for health and is
  refused when the config has errors, the slot has no launch section, another GPU slot runs, a managed server already
  runs, or the endpoint is not on this machine. Stop signals a process only when it still runs the recorded server
  binary; a stale or reused PID is cleared without being signalled. A responding endpoint is never taken as proof that
  Bridle manages it.
- One background operation runs at a time.
- There is no command runner and no way to change a policy verdict. A blocked decision is shown with its rule and reason;
  the policy fails closed and `ask` counts as deny.
- Event logs are read from the newest 2 MiB of each file, at most 5000 events. Unreadable lines are counted and shown in
  a banner. Text from logs is redacted again and stripped of control characters before display.

## Terminal notes

- Minimum size 60x14. Below 100 columns the navigation is a single row.
- `NO_COLOR=1` gives a monochrome theme and `LOCAL_AGENT_TUI_ASCII=1` replaces Unicode symbols. Status is always a symbol
  plus a word.
- Verified on Linux (WSL2) in a pseudo-terminal with real hook output, and with a stand-in server for start and stop. Not
  yet run on native Windows.
