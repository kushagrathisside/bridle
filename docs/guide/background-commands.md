# Notices and background commands

Two small features that belong together. A **background command** runs while the model goes on working. A **notice** is how Bridle tells
the model that something happened meanwhile. The design, and where the idea comes from (AWS's Infinity), is in
[ADR-0023](../decisions/0023-notices-background-commands-and-lanes-from-infinity.md).

## Notices

After a tool result, Bridle may add a block:

```text
[[bridle notices 4433832c4979: added by Bridle after the tool finished, not part of its output]]
1. [background_finished] bg-1 `git --version` passed after 0 s. Read its output with command_status bg-1.
[[end of bridle notices 4433832c4979]]
```

- **Always on.** There is nothing to switch on: a block appears only when something happened. The server's instructions (and the chat's
  system prompt) tell the model how a notice looks and give it this server's marker, twelve random hexadecimal digits.
- **Cannot be forged.** Before the block is added, every tool result is scrubbed of the marker and of the words that open and close the
  frame. A file, a commit message or a web page that contains a fake notice shows up as `[ [bridle notices ...` and is not one.
- **A hint, not the record.** A notice can be lost (Bridle ended, too many were waiting). It always names the tool that shows the real
  state.
- **Bounded.** At most 32 wait, 400 characters each, 8 are shown at once; dropped ones are counted in the block.

What can produce a notice today:

| Kind | When |
|---|---|
| `background_finished` | A command started with `command_start` ended, was stopped, or reached its time limit |
| `file_changed`, `file_gone` | A file the model read or wrote with the file tools was changed (or deleted) on disk by someone else since; the model's own writes do not count. Checked after every tool call, over the 256 most recent files, by size and time |

Board comments, lease releases and fleet events could use the same mechanism; none does yet.

## Background commands

Turn them on in the project or machine configuration:

```toml
[workflow]
background_commands = true      # offers command_start, command_status, command_stop
```

They are off by default because every tool costs prompt tokens on a small context.

| Tool | What it does |
|---|---|
| `command_start` | `program`, `args`, `cwd`, `timeout_ms` exactly as `run_command`, judged by the same policy and sandbox. Returns an id (`bg-1`) at once |
| `command_status` | With an id: the state (running, passed, failed, timeout, stopped), the exit code and the last 4 KB of output, redacted. `wait_ms` (up to 10 s) waits for it to end. Without an id: the list |
| `command_stop` | Ends a command Bridle started, with its whole process tree. Always allowed, so a runaway command can be stopped |

Limits: three at once, a time limit of 1 s to 10 min (default 5 min), 64 KiB of output kept, sixteen finished commands remembered. The
commands belong to the tool server of the workspace; when it ends they are stopped. If Bridle itself is killed without a chance to clean
up, a running command keeps going until it ends.

A command the policy would not run (a program that is not allowed, a path as the program, shell characters, a working directory outside
the project) is not started: the answer is the same refusal `run_command` gives.

## What was and was not shown

Run against the real `bridle mcp` over stdio: the instructions carry the marker; a command that ended is announced after the next result,
once; a file named like a notice frame is neutralised in `git_status`'s text; a command over its time limit is stopped; `command_stop`
works; a blocked program is not started; a long command does not outlive the server. **Not measured:** whether a 9B model reads a notice
correctly or uses `command_status` when it should. Infinity's authors say models are mostly trained on synchronous tool calls.

## Related

- [Lanes](lanes.md) · [Configuration](configuration.md) · [Security](../concepts/security.md)
