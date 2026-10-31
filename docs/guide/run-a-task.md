# Run a task

A task is the unit that Bridle tracks: one objective, the calls made towards it, and the decisions the policy took.
Tasks are how you read what the agent did and why. This page covers starting, inspecting, resuming and resetting a
task, running it in a worktree, and reading the record.

## Turn on the task workflow

The task workflow records each call in a journal and applies the [loop rules](../concepts/tasks-and-loops.md). It is off
in the default configuration. Turn it on in the project file:

```toml
[workflow]
enabled = true
```

The `local-only` profile does not enable it. The machine configuration may switch the workflow off, and the project file
may not. The keys and their defaults are listed in the [configuration reference](configuration.md).

Without the workflow, Bridle still checks every call against the policy and records each verdict in
`.local-agent/tool-events.jsonl`. Only the loop rules and the journal depend on the workflow.

## Start a task

```bat
agent task start --prompt "Fix the bug in calc.py so the unit tests pass."
```

The prompt is stored redacted. The next new OpenCode session in this repository works on the new task. The command
prints the task id and the next step:

```text
started task-0001; the next new OpenCode session works on it
```

To run the task in its own git worktree, add `--worktree`. Bridle creates `.worktrees/agent-task-NNN` on a new branch
from the current commit and binds the next session to it:

```text
started task-0002 in <repo>\.worktrees/agent-task-002
open OpenCode in that folder; its first session works on task-0002
```

The worktree needs a git repository with at least one commit. Changes made in the worktree stay on its branch until you
merge them.

## Inspect a task

```bat
agent task status
agent task status task-0001 --json
```

`task status` prints the phase, the status, the counters and the number of records:

```text
task: task-0001
phase: IMPLEMENT
status: Active
iteration: 0
last action: edit calc.py
last test: none
failures in a row: 0
refusals in a row: 0
journal records: 7
```

`agent trace` prints the ordered journal, one line per record:

```text
     1  task_started task-0001 repository=...
     2  call_started tool=edit read_only=false verification=false edits=calc.py
     6  refused rule=repeated_edit
```

`agent explain` prints the recent decisions and the reason for each:

```text
task-0001: 1 decision(s) recorded, showing 1
call e5: AskUser because RepeatedEdit (confidence 1.00, provider rules), refused
  signals: RepeatedEdit { file: "calc.py", count: 4 }
```

Both commands take `--limit` to show more records or decisions, up to 1000.

## Resume, reset and repair

| Command | Use it to |
|---|---|
| `agent task resume <task>` | Bind the next new session to an existing task. |
| `agent task reset` | End a block and start the loop counters again. The journal is kept. |
| `agent task repair` | Rebuild the cached state from the journal, after a damaged file. |
| `agent task prune` | Remove the folders of finished tasks and their session bindings. Worktrees are not removed. |

`reset` is the way to continue after a task was blocked by the iteration limit. It does not change the files in the
repository.

## Read a decision

A refused call is recorded with its rule. For a repeated edit the record reads:

```text
refused by loop rule repeated_edit: this file was edited again and again since the last verification.
Allowed next: run the verification before editing the file again.
```

The [tasks and loops](../concepts/tasks-and-loops.md) page lists every rule, its default threshold, and what the agent is
told to do next.
