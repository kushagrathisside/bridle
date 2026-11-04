# ADR-0023: Notices, background commands and lanes, taken from Infinity and kept inside Bridle's rules

- **Status:** Accepted (2026-11-04); the three parts are built and tested; the lanes are not yet used by the board runner
- **Date:** 2026-11-04
- **Deciders:** Kushagra Srivastava

## Context

The owner pointed to AWS's open-source release of **Infinity** (Hydro project, Apache-2.0, blog post of 23 October 2026) and to
Google Research's **Harness²** (paper arXiv 2610.14224), and asked what Bridle can learn. The reading is in
[Lessons from Infinity and Harness²](../spec/lessons-from-infinity-and-harness2.md). This ADR records what was built from Infinity
and why the rest was not. (Harness² produced a proposal, not code; see the lessons note.)

Infinity's headline, tens of thousands of idle agents in 8 GB, does not transfer: Bridle is limited by video memory and one 9B model,
not by the memory of an idle agent. Three of its design ideas do, because they are about **how an agent and its surroundings talk to
each other**, and that is Bridle's problem too.

## Decisions

### 1. Notices: the runtime tells the model what happened, in a block no tool can forge

Infinity shows an asynchronous event to the model as a tool call that the runtime injected and that the model cannot make itself.
Bridle's tool server cannot push into a conversation (MCP is request and reply), so a notice is added to the **next tool result**:

- `harness_runtime::notices` holds a bounded queue per workspace (32 waiting, 400 characters each, 8 shown at once; what is dropped is
  counted and said).
- `Server::call_tool` runs the tool, then looks for files that changed under the model, **scrubs** the tool's text, and appends the
  block. The block is framed `[[bridle notices <marker>: ...]] ... [[end of bridle notices <marker>]]` where the marker is twelve
  random hexadecimal digits made for each server and told to the model once, in the server's instructions (and, for the chat, in its
  system prompt).
- **Forgery.** The scrub removes the marker from every tool result and turns any `[[bridle notices` or `[[end of bridle notices` in it
  into `[ [bridle notices ...`, so text in a file or a web page cannot pose as a notice, with or without guessing the marker.
  Notice texts are redacted like any output; the frame is not, so the marker stays exactly what the instructions say.
- **A notice is a hint, not the record.** It can be lost (the process ended, the queue was full). The state it announces can always
  be read with a tool, and the notice names that tool.

Two producers exist. A **background command ending** (decision 2). A **file changing on disk after the model read it**: the file
tools already remember the hash of what they last read or wrote; they now also remember its size and time, and after every tool call the
server checks at most 256 recent files by size and time, reads one only when those differ, and reports a real change (or a deletion)
once per read. The model's own writes do not count. This moves the old stale-write refusal from "when you try to write" to "as soon as
it happened".

*Not wired:* board comments, lease releases and fleet events could post notices the same way; none does yet.

### 2. Background commands: `command_start`, `command_status`, `command_stop`

A command that takes minutes (a build, a test suite) need not hold the model still. `command_start` takes exactly what `run_command`
takes and judges it with the same function (`run::authorize`) and the same sandbox, so a command the policy would not run is not
started here either. It returns an id (`bg-1`) at once. When the command ends, a notice follows the next result. `command_status`
shows the state, the exit code and the last 4 KB of output (redacted), and can wait up to 10 seconds. `command_stop` ends the whole
process tree and is allowed like `browser_close`, so a runaway command can always be stopped.

Bounds: three at once per workspace, a time limit of one second to ten minutes (default five), 64 KiB of output kept, sixteen finished
commands remembered. The commands belong to the tool server of the workspace and are stopped when it ends.

The tools are **off by default** (`[workflow] background_commands = false`), for the reason `task_status` is: each tool schema costs
prompt tokens on a small context. Infinity's authors say models are mostly trained on synchronous tool traces and that reasoning about
interleaved events is an open question; whether a 9B model uses these tools well is **not measured**.

*Known limit:* if Bridle is killed without a chance to clean up, a running command keeps running until it ends, and its time limit,
which Bridle enforces, is gone with Bridle.

### 3. Lanes: parallel members in separate worktrees, merged by the person

Infinity Code edits in parallel in stacked sandboxes and presents each result as a diff. Bridle's version, for the plan's unbuilt
"worktrees for parallel engineers and integration" (F1):

- `bridle lanes new NAME [--from LANE]` snapshots the project's working tree (uncommitted changes included; ignored and secret files
  left out, as in a checkpoint) and checks it out as a Git worktree in `.bridle/lanes/NAME/`. A member works in it by being given that
  folder as its workspace; it then has its own leases, its own run lock and its own `.bridle/`.
- `bridle lanes diff NAME` shows what the lane changed against its snapshot. This is the review.
- `bridle lanes integrate NAME... [--dry-run] [--skip-conflicts]` snapshots the project as it is **now**, merges the lanes onto it in
  the order given with `git merge-tree` (three-way, touching nothing), and only when nothing conflicts writes the merged state into the
  working tree, with the rule of Undo: a file you changed since the snapshot is left alone and listed. The project is checkpointed
  before and after; `bridle lanes undo` puts it back.
- A lane can start from another lane's work, so one member can build on another's unfinished edit.

**The model never integrates.** It is a command for the person (and later a button). Two lanes cannot conflict while they work, because
they are different folders; they conflict, visibly, at the merge. Lanes need a real Git repository: a folder with only Bridle's shadow
repository cannot have worktrees, and the command says so.

## What was not taken

- **Infinity's runtime and RAP.** It is built for Lambda, SQS and Aurora; its tool execution has no sandbox or security model that the
  post describes, and Bridle's policy layer is the reason to run tools here at all.
- **Priority scheduling, a tree of agents with compaction.** The authors list them as open; Bridle's scheduler is deliberately simple.
- **Self-modifying harnesses (Harness²) at run time.** See the lessons note: the useful part is an offline, verifier-backed, human-approved
  loop over prompts, not a live one.

## Verified, and not

- Notices, scrubbing, the file-change check, the background tools (start, finish, fail, stop, time limit, refusals, the limit of
  three, stop-all) and the lane operations (two lanes merging, a conflict that writes nothing, `--skip-conflicts`, a dry run, undo,
  stacked lanes, removal) are covered by tests; the lane tests use real temporary Git repositories. See the progress entry for the
  numbers and the live checks.
- **Measured with a real model** (`bridle eval-agent`, Qwen3.5-9B, five runs per scenario; small samples):
  - *Told to start a background command and read it with `command_status`:* it did, 5 of 5 (after the scoring was corrected, see below).
  - *Told only that Bridle will announce the end:* it saw the notice and read the result 4 of 5; in the fifth the command ended before any
    other result carried the notice, and the model never read it.
  - *Polling:* the model called `command_status` over and over (up to 21 times in one run) instead of waiting. The tool's description and
    its answer for a running command now say "do not poll: pass `wait_ms` or carry on"; in the five runs after that change the calls
    dropped to 0 to 4 for each run. The two sets of runs also differ in the scoring and the machine's timing, so this is an indication, not a result.
  - *A file changed by the person after the model read it:* the notice arrived after the next call and the model re-read the file 5 of 5
    and kept both edits 5 of 5. **But the control, with nobody changing the file, had the same edit failures** (`old_string was not found`:
    the model copies the line-number column of `read_file` into its edit), so these runs do not show that the notice saved anything. They show
    that the notice is delivered and that the model reacts to it.
  - *Scoring correction, disclosed:* the first run scored the answers on what `ping` printed, but a ping to this machine times out inside the
    sandbox, so all 10 background runs failed on the check and not on the model. The check was widened to accept any report of the outcome and
    the runs repeated; the first run's files are kept (`ev-notices-first`).
  - *The project manager's plan, ten runs:* all three conditions 7 of 10 (see the lessons note).
  - *Two real turns in two lanes:* each touched only its own file and `integrate` merged them, no conflict.
- **Built since this record was first written:** notices for a refused write that is free now (`lease_free`) and for the person changing the
  issue a member is working on (`board_change`); records of running background commands so `doctor --repair` stops orphans.
  **Not built:** lanes inside the board runner (a member is not yet given a lane automatically, and the runner runs one turn at a time), a
  button for integration in the window, fleet notices.

## Related

- [Lessons from Infinity and Harness²](../spec/lessons-from-infinity-and-harness2.md)
- [Notices and background commands (guide)](../guide/background-commands.md) · [Lanes (guide)](../guide/lanes.md)
- [Runs, checkpoints and recovery](../reference/run-recovery.md)
