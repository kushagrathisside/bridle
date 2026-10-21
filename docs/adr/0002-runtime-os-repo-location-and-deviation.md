# ADR-0002: Runtime OS support, repository location, git client, and a deviation from R-6.1

| | |
|---|---|
| **Status** | Accepted (decisions D1, D3, D4, D5, D9, D13 in the M1 plan, answered by the project lead on 2026-10-21) |
| **Date** | 2026-10-21 |
| **Supersedes** | Nothing. Amends the environment assumptions in ADR-0001 and the statement in R-6.1 |
| **Deviation** | Records a departure from R-6.1 and R-6.4 (brief section 6). Under R-62.3 this is a strategic change, so the justification and the risks are stated here |

## Context

The brief (section 6) makes native Windows the default inference environment and forbids moving the
inference stack into WSL for tooling preference (R-6.1, R-6.4). WSL remains an escape hatch for repositories
that need Linux.

Facts that affect the decision, all verified on this machine:
- Laptop GPU: RTX 4060 Laptop, 8188 MiB, driver 592.82. Both Windows and WSL can see it.
- RAM: 16 GB. WSL is capped at 11 GB by `.wslconfig` (`memory=11GB`), and that cap does not bound Windows processes.
- Networking: WSL uses NAT by default. Windows `127.0.0.1` services are not reachable from WSL's own `127.0.0.1`.
- Disk: C: has about 60 GB free. The WSL disk holds about 280 GB of data after cache cleanup. Its virtual disk file
  lives on C: and only shrinks after compaction.
- Toolchain: Windows has rustup 1.95.0 (MSVC), VS 2022 Community (MSVC 14.44), and the Windows SDK. WSL has
  rustup 1.95.0 (GNU). `link.exe` is not on PATH without the VS developer environment.
- Only Windows llama.cpp CUDA binaries are downloaded, so a WSL run needs either those binaries through WSL
  interop or a Linux CUDA build.

## Decision

1. **Both runtimes are supported.** Model servers can run natively on Windows or inside WSL. Both paths pass the
   same argv, health and completion tests.
2. **The default is set by a proof-of-concept (POC) race.** The first runtime to pass the POC on the critical path
   (launch, `/health`, one completion, one Qwen tool call, memory readings) becomes the default in configuration.
   The other stays supported.
3. **The repository stays in WSL ext4.** Moving it to D: frees only about 3 GB of C:, and cargo and git on
   `/mnt/d` from WSL are slow. The disk pressure comes from the WSL disk as a whole and is handled by cache
   cleanup and compaction, not by moving the repo.
4. **Windows builds use an external target directory on D:** `D:\cargo-target\harness`. WSL builds keep the repo's own
   target directory. Each OS has its own target directory, so the two toolchains never share artifacts.
5. **One git client for the repo: WSL git.** Windows builds read the repository through `\\wsl.localhost\Ubuntu\...`.
   P1 checks that this works. If it does not, the fallback is a mirror on D:, synced from WSL.
6. **WSL networking is set to mirrored mode** (`networkingMode=mirrored` in `.wslconfig`). This makes Windows
   `127.0.0.1` services reachable from WSL, so the loopback-only endpoint policy (T06) still holds. Applying it
   requires `wsl --shutdown`, which stops running WSL work, so it runs at a time the project lead chooses.
   Compaction of the WSL virtual disk is batched into the same shutdown.

## Deviation from R-6.1 and R-6.4

R-6.1 says inference runs natively on Windows, and WSL is not the default. Under decision 2, WSL can become the
default if its POC passes first. That departs from the brief. The departure is accepted under the project lead's
answer on 2026-10-21, with these conditions:
- The default is recorded in configuration with the POC evidence (timings, memory, pass/fail) attached.
- Changing the default later needs a new POC record and a note in this ADR.
- Both runtimes stay in CI-independent tests (native Windows and WSL), with GPU runs kept manual.

## Consequences and risks

| Risk | Mitigation |
|---|---|
| WSL memory cap (11 GB) limits WSL servers | POC measures memory; cap stays as configured |
| Windows build reads `\\wsl.localhost`, which may be slow or fail | P1 checks it; mirror on D: is the fallback |
| Two toolchains on the same machine | One target directory per OS; `CARGO_TARGET_DIR` set only in the Windows environment |
| Mirrored networking needs a WSL restart | Batched with compaction; run at a time the project lead chooses |
| Repo move would have broken agent worktree paths | Repo stays put, so worktrees are unaffected |

## Alternatives considered

- **Native Windows only.** Follows the brief exactly. Rejected by the project lead, who wants WSL to remain a
  supported runtime and to compete on the POC.
- **WSL only.** Requires a Linux CUDA build and changes the default much further from the brief.
- **Move the repo to D:.** Rejected after measurement: it frees about 3 GB and slows WSL builds.

## Verification

- P1 (Windows) and P2 (WSL) each log: launch, `/health` 200, one completion, one Qwen tool call, memory readings,
  and the binary used. The result file is attached to this ADR.
- `cargo test --workspace` passes on native Windows (T17) and in WSL, against the same commit.
