# ADR-0016: The Linux sandbox for commands the harness runs is Landlock, applied before the command starts

- **Status:** Accepted (2026-10-31); first slice built the same day (progress.md)
- **Date:** 2026-10-31
- **Deciders:** Kushagra Srivastava

## Context

The policy is not a sandbox (core-security.md, residual risks). An allowed command, such as a test run or a verification
step, runs with the user's full access: it can read the user's SSH keys and cloud credentials, write outside the
repository, and open network connections. ADR-0012 proposed a Windows sandbox and left Linux open. The owner has now chosen
native Ubuntu first (ADR-0014), so the Linux mechanism is decided here.

Options considered on Linux:

- **Landlock** (kernel 5.13 and later; network rules from ABI 4, kernel 6.7 and later). No root, no installed program, no
  new privileged daemon. The kernel on this machine supports ABI 7 and has Landlock built in.
- **bubblewrap** (`bwrap`). Strong, but it needs a program installed on each machine, and it is not installed here.
- **Mount namespaces** through `unshare`. Needs privileges or user namespaces, which some distributions disable.

## Decision

1. **Commands run under Landlock.** The harness applies a Landlock ruleset to the child process before it starts, through
   `pre_exec`. The commands are `run_command`, the verification steps and `run_tests`, `run_linter` and `run_typechecker`.
   The git and model-server processes are the harness's own tools and are not confined by this decision.
2. **Writes** are allowed in the repository, in `/tmp` and `/var/tmp` (where tests and build tools put scratch files), and
   to `/dev/null`. Nothing else is writable.
3. **Reads** are allowed in the repository, the system directories (`/usr`, `/lib`, `/lib64`, `/bin`, `/sbin`, `/etc`,
   `/proc`, `/sys`, `/dev`), and the toolchain directories in the user's home (`~/.cargo`, `~/.rustup`), when they exist.
   The rest of the home directory is unreadable, which keeps SSH keys and cloud credentials out of reach. Git inside the
   sandbox runs with `GIT_CONFIG_GLOBAL=/dev/null` and `XDG_CONFIG_HOME=/dev/null`: the user's global git configuration and
   default ignore file name files in the home directory, and neither is read there. The system git configuration still
   applies.
4. **Network is denied** (TCP bind and connect) unless `[sandbox] network = true`. Denying network needs ABI 4; on an older
   kernel the command is refused, not run unconfined.
5. **Fails closed.** If the kernel does not support Landlock, or the ruleset cannot be applied, the command is refused and the
   refusal says why. There is no fall-back to running unconfined.
6. **Configuration.** `[sandbox] enabled` (on by default on Linux, off elsewhere) and `network` (off by default). A
   repository may not set `enabled = false`: only the machine config may (the same rule as the workflow switch).
7. **The policy still runs first.** A command the policy blocks is still blocked. The sandbox only limits what an allowed
   command can reach.

## Consequences

- Builds and tests that download dependencies at run time need `network = true`, or their dependencies vendored. The
  Linux gate script runs with the vendored set on this machine; the native Ubuntu run needs the same choice.
- The sandbox does not cover the model's own process, which is the harness's host, nor the git and model-server processes.
- On Windows the sandbox is off: ADR-0012 still applies to that platform.

## Open

- Whether `/tmp` should be private per command. Left shared for now, since build tools expect it.
- The mechanism on Windows (ADR-0012).
