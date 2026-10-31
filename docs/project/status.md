# Status and limits

This page states what Bridle does today, on which machine it has been checked, and what it does not do yet. It is
updated when a result changes.

## Release

Bridle is a release candidate, version `0.1.0`. It has not been published as a release, and no packaged binary is
provided. You build it from source; see [install Bridle](../guide/installation.md).

This release runs on native Windows only. It has been checked on one machine: Windows 11, an NVIDIA RTX 4060 Laptop GPU with
8 GB of VRAM, and 16 GB of RAM. Other systems are not tested.

## What works

These were run on the tested machine.

- **The policy.** The allow, block and ask answers for file reads, secret files and shell commands, through the same hook
  OpenCode calls.
- **Loop refusals.** A repeated edit is refused with the rule named, once the workflow is on.
- **Tasks.** Starting a task, starting one in a worktree, reading its status, its trace and its decisions, and resetting
  it.
- **Configuration.** Setup, validation, the merged view and the list of loaded files.
- **The tools server.** The tool list is served over MCP.
- **The main model.** Starting the server, and the health checks pass with the server running.

## What is checked less

- **The interactive OpenCode window.** On the tested machine, on 2026-10-29, the window opened and stayed blank. The cause
  is not yet known. Non-interactive runs (`opencode run`) use the same plugin and the same tools. A full end-to-end run
  with a model is recorded for an earlier configuration, on 2026-10-22, and not for the current one.
- **Benchmarks.** The benchmark runner exists, and a small corpus has been run. The results are not summarised here,
  because the sample is too small to support a claim about any model or any configuration.

## Optional and off by default

| Feature | Default | Where it is set | State |
|---|---|---|---|
| Task workflow and loop refusals | Off in the default configuration | `workflow.enabled` | Works. |
| Micro model for stalled loops | Off | `decisions.provider`, `routing.enable_micro_decisions` | Built and tested. |
| Deep model escalation | Off | `routing.enable_deep_escalation` | Built. The GPU swap was run live on the tested machine. |
| Subagent tier | Off | `subagents.enabled` | Built and tested on a small sample. |
| Jev decision service | Off | `external.jev` | Built and tested against a local mock. Not checked against a live service. |
| LiteLLM gateway route | Off | `provider = "litellm"` on a slot | Checked against a local gateway on 2026-10-28. |

## Known limits

- **Windows only.** Linux and macOS are not supported in this release.
- **Not a sandbox.** Allowed commands run with your user's rights. See [the security model](../concepts/security.md).
- **Two settings are not enforced yet.** `security.require_worktree` is read by the configuration checks, but no code
  path requires a worktree. `verification.required_for_done` is read by no code path, so a task is not blocked from
  finishing when a verification has not passed. The validator's warning about the second one overstates what happens.
  Use worktree tasks and run your verification yourself until these are enforced.
- **The configuration display.** `config show` prints boolean values under secret-like keys as `[REDACTED]`. The setting
  itself is in effect. The display is wrong, and this is listed until it is fixed.
- **Interactive use.** See the note above on the interactive window.
- **Code index.** Bridle does not keep a persistent index of symbols. Code tools parse the files they need on each call.
- **Multi-agent work.** Bridle does not run several agents on one repository at once.
- **Other platforms.** A WSL runtime is planned for a later release. It is not built.
