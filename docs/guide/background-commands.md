# Notices and background commands

These two features belong together. A **background command** runs while the model continues to work. A **notice** is the means by which
Bridle tells the model that something has happened in the meantime. The design, and the source of the idea (AWS's Infinity), are described
in [ADR-0023](../decisions/0023-notices-background-commands-and-lanes-from-infinity.md).

## Notices

After a tool result, Bridle may append a block of the following form:

```text
[[bridle notices 4433832c4979: added by Bridle after the tool finished, not part of its output]]
1. [background_finished] bg-1 `git --version` passed after 0 s. Read its output with command_status bg-1.
[[end of bridle notices 4433832c4979]]
```

- **Always available.** Nothing needs to be enabled: a block appears only when there is something to report. The server's instructions
  (and the chat's system prompt) tell the model how a notice looks and give it the marker of this server, which is twelve random
  hexadecimal digits.
- **Resistant to forgery.** Before the block is appended, every tool result is scrubbed of the marker and of the words that open and close
  the frame. A file, a commit message or a web page that contains a counterfeit notice therefore appears as `[ [bridle notices ...` and is
  not mistaken for one.
- **A hint, not the record.** A notice can be lost, for example if Bridle ends or too many are waiting. It always names the tool that shows
  the true state.
- **Bounded.** At most 32 notices wait, each of at most 400 characters, and at most eight are shown at a time; any that were dropped are
  counted in the block.

The following events can produce a notice.

| Kind | When it is produced |
|---|---|
| `background_finished` | A command started with `command_start` ended, was stopped, or reached its time limit |
| `file_changed`, `file_gone` | A file that the model read or wrote with the file tools has since been changed, or deleted, on disk by someone else. The model's own writes do not count. The check runs after every tool call, covers the 256 most recent files, and compares size and time |
| `lease_free` | A write that the model was refused, because another run held the file, can now proceed: the holder finished or its claim lapsed |
| `board_change` | While a member's turn is running, the person changed the issue it is working on, or commented on it |

Events from the fleet could use the same mechanism, but none does so yet.

## Background commands

Enable them in the project or the machine configuration:

```toml
[workflow]
background_commands = true      # offers command_start, command_status and command_stop
```

They are disabled by default because every tool consumes prompt tokens, which is significant on a small context.

| Tool | Purpose |
|---|---|
| `command_start` | Accepts `program`, `args`, `cwd` and `timeout_ms` exactly as `run_command` does, and is judged by the same policy and sandbox. It returns an identifier (`bg-1`) at once |
| `command_status` | With an identifier, reports the state (running, passed, failed, timeout or stopped), the exit code and the last 4 KB of output, redacted; `wait_ms` (up to ten seconds) waits for the command to end. Without an identifier, it lists the commands |
| `command_stop` | Ends a command that Bridle started, together with its whole process tree. It is always permitted, so that a runaway command can be stopped |

The limits are three commands at one time, a time limit of between one second and ten minutes (five minutes by default), 64 KiB of output
retained, and sixteen finished commands remembered. The commands belong to the tool server of the workspace and are stopped when it ends. If
Bridle itself is killed without the opportunity to clean up, a running command continues until it finishes. For that reason each running
command leaves a small record in `.bridle\background\` (the program's name, never its arguments, and the process identifiers), and
`bridle doctor --repair` stops the commands whose Bridle has gone, after checking that each process identifier still belongs to that program.

A command that the policy would not allow (a program that is not permitted, a path given as the program, shell characters, or a working
directory outside the project) is not started; the answer is the same refusal that `run_command` gives.

## Evidence

Run against the real `bridle mcp` over standard input and output, the following were confirmed: the instructions carry the marker; a command
that ended is announced after the next result, once only; a file named like a notice frame is neutralised in the text of `git_status`; a
command over its time limit is stopped; `command_stop` works; a blocked program is not started; and a long command does not outlive the
server.

A real model (Qwen3.5-9B) was also run through scripted scenarios, five runs each. Told how to use the tools, it used `command_start` and
`command_status` correctly in all five runs. Told only that Bridle would announce the end of the command, it read the result after the
notice in four of five runs. It tended to call `command_status` repeatedly instead of waiting, up to 21 times in one run, so the tool's
description and its answer for a running command now say that it should not be polled; in five later runs the calls fell to between none and
four per run. These samples are small and the two sets of runs differed in timing, so they are indications and not conclusions. A file that
was changed under the model produced a notice, and the model re-read the file in all five runs; but a control run, in which nobody
changed the file, showed the same editing failures (the model copies the line-number column of `read_file` into its replacement text), so
the runs do not show that the notice prevented a failure.

## Related

- [Lanes](lanes.md) · [Configuration](configuration.md) · [Security](../concepts/security.md)
