# ADR-0012: An OS sandbox for commands the harness runs, on Windows first

- **Status:** Deferred (2026-10-31): the Windows spike waits until routing is measured. The Linux sandbox is decided and
  built in [ADR-0016](0016-linux-sandbox-for-commands-with-landlock.md).
- **Date:** 2026-10-31
- **Deciders:** Kushagra Srivastava

## Context

The policy is not a sandbox. `docs/reference/modules/core-security.md` says so: running tests runs code the model wrote,
and an allowed `python script.py` can do anything the user can. The policy stops obvious harm and keeps secrets out of
the model's context and the logs. It does not stop a program that the policy allowed from reading the user's profile or
opening a network connection.

Claude Code sandboxes its shell on macOS and Linux. Codex uses Seatbelt on macOS and Landlock with seccomp on Linux.
Release 1 runs on native Windows, so neither mechanism applies directly.

The Windows options, as I understand them. These are the mechanisms, not measured here:

| Mechanism | What it gives | What it costs |
|---|---|---|
| Job object | Kills the process tree; memory and CPU limits; no filesystem or network isolation | Small; limits only |
| AppContainer | Filesystem and network isolation through capabilities; a process launched with the container's token | Needs a profile and explicit capabilities for the repository and the toolchain; more setup |
| Restricted token | Removes privileges; some file access checks | Weaker than AppContainer; hard to reason about |
| Windows Sandbox | A disposable VM | Not scriptable for one command at a time |
| WSL2 with bubblewrap | Mount and network namespaces | Release 2 route ([ADR-0002](0002-runtime-os-repo-location-and-deviation.md)) |

## Decision (proposed)

1. **Start with a Job object plus AppContainer** for `run_command`, `run_tests` and the verification steps. The Job object
   makes a stopped command take its children with it; whether the current stop path does this has not been checked.
   AppContainer limits file and network access to the repository and the declared toolchain paths.
2. **Spike before the design is final.** A spike runs `cargo test` and a script that reads a file outside the
   repository and opens a socket, inside the container. The result is recorded in `progress.md`.
3. **The sandbox adds to the policy; it does not replace it.** A command the policy blocks is still blocked. The sandbox
   only limits what an allowed command can reach.
4. **Failure is closed.** If the container cannot be created, the command is refused and the reason is logged. There is
   no fall-back to running unsandboxed.

## Consequences

- Build and test commands need the paths they use declared. A missing path makes the command fail visibly, not silently.
- Network access from tests will need an explicit allowance per project, recorded in config and checked by V-rules.
- The mechanism is Windows-specific. Linux and macOS need their own mechanisms; release 1 does not need them.

## Decided (2026-10-31, by the owner)

- **The spike waits.** It runs after the routing work is measured (ADR-0010), not now. The mechanism stays as proposed:
  Job object plus AppContainer, with the spike first.
- Still open, to be decided before the spike: which toolchain paths the container may use, and whether tests may reach
  the network. The proposal default is no network.
