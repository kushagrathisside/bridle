# ADR-0002: Runtime OS support, repository location, git client, and a deviation from the original design

- **Status:** Accepted on 2026-10-21, with the decisions answered by the project lead. Amended on 2026-10-21: release 1
  is Windows-only (see [Amendment](#amendment-2026-10-21-release-1-is-windows-only)). Amended on 2026-10-31 by
  [ADR-0014](0014-linux-native-first-then-windows.md): the first working release is native Ubuntu, and the Windows release
  follows. The runtime decision in this record stands.
- **Date:** 2026-10-21
- **Deciders:** Project maintainer
- **Supersedes:** Nothing. Amends the environment assumptions in [ADR-0001](0001-rust-core-thin-host-adapter.md)
  and the earlier statement that inference runs natively on Windows.
- **Deviation:** Records a departure from the original runtime assumption that inference run natively on Windows. A change of this kind
  is strategic, so the justification and the risks are stated here.

## Context

The original design makes native Windows the default inference environment. It forbids moving the
inference stack into WSL for tooling preference. WSL remains an escape hatch for repositories
that need Linux.

These facts affect the decision. All were verified on this machine:

- Laptop GPU: RTX 4060 Laptop, 8188 MiB, driver 592.82. Both Windows and WSL can see it.
- RAM: 16 GB. `.wslconfig` caps WSL at 11 GB (`memory=11GB`). That cap does not bound Windows processes.
- Networking: WSL uses NAT by default. Services on Windows `127.0.0.1` are not reachable from WSL's own
  `127.0.0.1`.
- Disk: C: has about 60 GB free. The WSL disk holds about 280 GB of data after cache cleanup. Its virtual disk
  file lives on C: and only shrinks after compaction.
- Toolchain: Windows has rustup 1.95.0 (MSVC), VS 2022 Community (MSVC 14.44) and the Windows SDK. WSL has
  rustup 1.95.0 (GNU). `link.exe` is not on PATH without the VS developer environment.
- Only the Windows llama.cpp CUDA binaries are downloaded. A WSL run therefore needs either those binaries
  through WSL interop or a Linux CUDA build.

## Decision

1. **Both runtimes are supported.** Model servers can run natively on Windows or inside WSL. Both paths pass
   the same argv, health and completion tests.
2. **A proof-of-concept (POC) race sets the default.** The first runtime to pass the POC on the critical path
   (launch, `/health`, one completion, one Qwen tool call, memory readings) becomes the default in
   configuration. The other stays supported.
3. **The repository stays on the WSL Linux filesystem.** Cargo and git are slow on the Windows drives mounted under
   `/mnt/`, and moving the repository would free little space. Disk pressure is handled by cache cleanup.
4. **Windows builds use a target directory outside the repository.** WSL builds keep the repository's own target
   directory. Each OS has its own target directory, so the two toolchains never share
   build artifacts.
5. **One git client for the repository: WSL git.** Windows builds read the repository through
   `\\wsl.localhost\Ubuntu\...`. The proof of concept checks that this works. If it does not, the fallback is a
   mirror on the Windows drive, synced from WSL.
6. **WSL networking is set to mirrored mode** (`networkingMode=mirrored` in `.wslconfig`). This makes services
   on Windows `127.0.0.1` reachable from WSL, so the loopback-only endpoint policy still holds. Applying
   it requires `wsl --shutdown`, which stops running WSL work, so it runs at a time the maintainer chooses.
   Compaction of the WSL virtual disk is batched into the same shutdown.

### Deviation from the original runtime assumptions

The original design says inference runs natively on Windows and that WSL is not the default. Under decision 2,
WSL can become the default if its proof of concept passes first. That departs from the original design. The departure is
accepted under the decision on 2026-10-21, with these conditions:

- The default is recorded in configuration, with the POC evidence (timings, memory, pass or fail) attached.
- Changing the default later needs a new POC record and a note in this ADR.
- Both runtimes stay in tests that do not depend on CI (native Windows and WSL). GPU runs stay manual.

### Amendment (2026-10-21): this release is Windows-only

The first working release targets Windows, and the WSL runtime follows in a later release. The Windows path is
complete: the proof of concept passed, the Windows build and tests run, and the end-to-end loop ran through OpenCode on
Windows. The WSL path still needs mirrored networking, a WSL shutdown, and either a Linux CUDA build or interop, so it
is deferred to a later release. The decision to support both
runtimes stays. Only the release scope changes. The deviation from the original runtime assumption is therefore
limited to the planned WSL support, which remains unbuilt.

## Consequences

| Risk | Mitigation |
|---|---|
| The WSL memory cap (11 GB) limits servers in WSL. | The POC measures memory. The cap stays as configured. |
| The Windows build reads `\\wsl.localhost`, which may be slow or fail. | The proof of concept checks it. A mirror on the Windows drive is the fallback. |
| Two toolchains run on the same machine. | One target directory per OS. `CARGO_TARGET_DIR` is set only in the Windows environment. |
| Mirrored networking needs a WSL restart. | Batched with compaction, and run at a time the maintainer chooses. |
| A repository move would have broken agent worktree paths. | The repository stays put, so worktrees are unaffected. |

### Verification

- The Windows and WSL proofs of concept each log the launch, `/health` 200, one completion, one Qwen tool call,
  memory readings, and the binary used. The result file is attached to this ADR.
- `cargo test --workspace` passes on native Windows and in WSL, against the same commit.

### Proof-of-concept result (native Windows, 2026-10-21)

Binary: llama.cpp b11756 CUDA 12.4 (`llama-server.exe`). Model: Qwen3.5-9B Q4_K_M (checksum verified).
Argv: `-m … --host 127.0.0.1 --port 8001 --alias qwen3.5-9b -c 16384 --jinja --metrics -ngl 99 -fa on
-ctk q8_0 -ctv q8_0 -np 1 --cache-ram 1024 --load-mode mmap --reasoning-budget 0`.
Script and raw output: the proof-of-concept run script and its output file, kept with the Windows runtime folder.

| Check | Result |
|---|---|
| Health `/health` 200 | Yes, after 11 s |
| VRAM after load | 5443 MiB of 8188 MiB |
| Completion (`READY`) | Correct |
| Tool call (`get_weather`, `city: Paris`) | Correct; `finish_reason: tool_calls` |
| Server RSS | 3494 MB |
| Host free RAM with server running | 382 MB of 16088 MB |
| Host free RAM after stop | 3834 MB; `vmmemWSL` holds about 3493 MB while idle |

On this 16 GB host, the server plus the WSL VM leaves almost no headroom. The first full Qwen run must close
other heavy applications first, and the WSL memory cap (or a WSL shutdown) becomes part of the run procedure.

## Alternatives considered

- **Native Windows only.** Follows the original design exactly. Rejected, because WSL should remain a
  supported runtime and to compete in the proof of concept.
- **WSL only.** Requires a Linux CUDA build and moves the default much further from the original design.
- **Move the repository to the Windows drive.** Rejected after measurement: it frees little space and slows WSL builds.

## Related

- [ADR-0001](0001-rust-core-thin-host-adapter.md): Rust core and thin host adapter. This ADR amends its
  environment assumptions.
- [ADR-0003](0003-tree-sitter-for-code-intelligence.md): its grammars need the MSVC build environment that the
  Windows build scripts load.
- [Installation](../guide/installation.md).
