# Status and limits

This page states what Bridle does today, on which machine it has been checked, and what it does not do yet. It is updated when a result
changes. The detailed record, with dates and evidence, is [progress.md](../status/progress.md).

## Release

Bridle is version `0.1.0`, a candidate for a first release. A release zip can be built and checked (see [releasing](releasing.md)); none has
been published yet. This release runs on **native Windows only**. It has been checked on **one machine**: Windows 11, an NVIDIA RTX 4060
Laptop GPU with 8 GB of VRAM, and 16 GB of RAM. Other systems are not tested. The program is not code-signed.

## What works

These were run on the tested machine; each has tests, and most have also been run live.

- **The policy.** The allow, block and ask answers for file reads, secret files, shell commands and the network, through the same
  decision path for the window, the terminal UI and OpenCode.
- **The window.** Chat, editor, diffs, search, terminal, images, context compaction and the project switcher.
- **The team and the board.** Named members, the board and the ideas queue, with a Git layer under each run: checkpoints before and after,
  undo, a run lock, recovery of a run cut short, leases and a stale-write check, and a shadow repository for folders without Git.
- **Tasks and loop refusals.** Starting a task, a worktree task, its status, trace and decisions, and resetting it.
- **Configuration.** Setup, validation, the merged view and the list of loaded files.
- **The tools server.** The tool list is served over MCP.
- **The models.** Starting a server, the health checks, the video-memory guard, and the one-GPU-model-at-a-time rule.

## Built, and checked less

| Feature | Default | State |
|---|---|---|
| Web research | Off | Tools and checks tested against local fake servers. The search providers have never talked to the real services. |
| The managed browser | Off, optional install | Run for real against Chromium on one machine and one company proxy; the DNS-rebinding defence is shown against a scripted resolver, not a hostile DNS server. |
| Window screenshots | Off | Tried on one program on one display. |
| The small-model fleet and its scheduler | Off | Run on the real models: eight tasks about 3.7 times faster than one at a time, the swap and its crash recovery shown. The small models' answers were not graded. |
| Lanes | Always available | Tested on real Git repositories and run through the built program, including two real model turns in two lanes. The board runner does not give members lanes yet. |
| Notices and background commands | Notices always on; background commands off | Tested and run through the built program. With a real 9B model, five runs per scenario: it saw and acted on a notice in 4 to 5 runs of 5, and it polled `command_status` more than it should (the tool now tells it to wait). |
| Task workflow and loop refusals | Off in the default configuration | Works. |
| Micro model for stalled loops | Off | Built and tested. |
| Deep model escalation | Off | Built. The GPU swap was run live. |
| Subagent tier | Off | Built and tested on a small sample. |
| Jev decision service | Off | Built and tested against a local mock; not checked against a live service. |
| LiteLLM gateway route | Off | Checked against a local gateway on 2026-10-28. |

## Known limits

- **Windows only.** Linux and macOS are not supported in this release. A WSL runtime is planned and not built.
- **Not a sandbox.** Allowed commands run with your user's rights. See [the security model](../concepts/security.md).
- **No approvals.** `ask` is never allowed, so network, destructive and risky commands are denied, not offered for confirmation.
- **One conversation at a time** in the window, and images are limited to 4 MiB.
- **The team works one member at a time.** One model answers every member, in the project's working copy. How well a 9B model plays each
  role is barely measured: the project manager created both child issues in only one of two early runs.
- **Two settings are not enforced yet.** `security.require_worktree` is read by the configuration checks, but no code path requires a
  worktree. `verification.required_for_done` is read by no code path, so a task is not blocked from finishing when a verification has not
  passed. Use worktree tasks and run your verification yourself until these are enforced.
- **The configuration display.** `config show` prints boolean values under secret-like keys as `[REDACTED]`. The setting itself is in
  effect; the display is wrong.
- **Code index.** Bridle does not keep a persistent index of symbols; code tools parse the files they need on each call.
- **The interactive OpenCode window.** On 2026-10-29 the OpenCode window opened and stayed blank on the tested machine; the cause is not
  known. Non-interactive runs (`opencode run`) use the same plugin and tools, and Bridle's own window is not affected.
- **Benchmarks.** The runner exists and a small corpus has been run; the sample is too small to support a claim about any model.
- **Downloads and OpenCode's catalogue.** Model downloads, and OpenCode's own model-catalogue fetch, are outside Bridle's control.
