# ADR-0014: Native Ubuntu is the first working release target; Windows follows

- **Status:** Accepted (2026-10-31)
- **Date:** 2026-10-31
- **Deciders:** Kushagra Srivastava

## Context

ADR-0002 made native Windows the default inference environment, with WSL deferred to release 2. Work since then has
shown that the Windows path carries most of the friction: Cargo on Windows could not reach crates.io and clippy ignored
the vendored-source file; WSL could not reach model servers on the Windows loopback; proxy variables broke loopback
tests; and paths cross `/mnt/d`. A native Ubuntu machine with 16 GB of RAM avoids every one of these layers.

The Linux code already passes the gate: formatting, clippy with warnings as errors, 855 tests, and a release build.
The Windows gate has not run on the current branch.

The Ubuntu machine has no discrete GPU. The main model will run on CPU there, so its speed is unknown until measured.

## Decision

1. **Release 1 is a working Bridle on native Ubuntu first.** Build, gate and run on that machine, without WSL and without
   Windows in the loop.
2. **The Windows release follows** from the same code, with its own gate and smoke test, once the Linux release works.
   The Windows machine is the GPU host for the main model; WSL is used only for tasks that need Linux.
3. **The runtime paths are machine settings.** Shipped profiles name the Windows binaries (`llama-server.exe`). A Linux
   machine overrides them with an explicit config file, passed through `--config` or `LOCAL_AGENT_CONFIG`. That file sits
   above the repository layer, so the repository stays unchanged. The example is in
   `configs/examples/linux-machine.toml`.
4. **The gate is one script.** `scripts/linux-gate.sh` runs the format check, clippy, the full tests and the release build,
   and runs a smoke test on the binary. Proxy variables are cleared for the tests only.
5. **Performance is measured before it is claimed.** The CPU speed of the micro model and of the main model on that machine
   is recorded in `progress.md`, with the pass line set before the run.

## Consequences

- ADR-0002 still holds for the inference-stack decision. Only the release order changes: Linux comes first.
- The Linux release has no GPU. The main model's speed will be well below the RTX 4060 figures, and the small models carry
  the daily work until measured.
- Two machines will run the same branch. Machine-specific paths stay in each machine's config file.
- The Windows release is not dropped. It is ordered after the Linux one.

## Open

- Whether the Ubuntu machine serves model endpoints to the laptop over the network. The endpoint policy allows only
  loopback unless cloud models are enabled (rule V4), so that would need its own decision.
- The integrated GPU. A Vulkan build of llama.cpp might use it; nothing has tested that, so it is not assumed.
