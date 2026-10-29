# Installation on Windows

This page takes you from a clean Windows machine to a project that runs OpenCode through the Bridle harness with
the `main` model slot. It is for an engineer setting up release 1. It assumes you can run commands in a Windows
shell and have downloaded the model weights listed in [models.md](models.md).

> **Status:** Release 1 (native Windows). Verified on one machine (Windows 11, RTX 4060 Laptop 8 GB, 16 GB RAM); other
> setups are not tested. End-to-end runs verified on 2026-10-21 and 2026-10-22.

## Prerequisites

| Item | Version used | Notes |
|---|---|---|
| Windows 11 | 10.0.26200 | native build and run |
| Visual Studio 2022 (Community) with the C++ workload | MSVC 14.44 | the `vcvars64.bat` environment is needed for `link.exe` |
| Rust | 1.95.0, MSVC host | pinned by `rust-toolchain.toml` |
| OpenCode | 1.19.3 (`opencode-windows-x64.zip`) | portable; SHA-256 `e4ce9117…20d8c3` |
| llama.cpp | b11756, CUDA 12.4 (`llama-b11756-bin-win-cuda-12.4-x64.zip`) | plus `cudart-llama-bin-win-cuda-12.4-x64.zip` |
| NVIDIA driver | 592.82 (CUDA 13.1 capable) | `nvidia-smi` must work |
| Python | 3.13 on PATH | used by the example project's tests |

The `deep` slot (Ternary Bonsai 2 27B) needs the PrismML llama.cpp fork (`prism-b10787-73bfe6c`, CUDA 12.4). The
harness runs without the fork, and the deep slot is then unavailable.

## Directory layout

Place the files as follows. The `micro` and `deep` entries are for the micro and deep slots, which run only when their
weights and runtimes are present and configured.

```text
D:\models\Qwen3.5-9B\Qwen3.5-9B-Q4_K_M.gguf
D:\models\LFM2.5-1.2B-Thinking\LFM2.5-1.2B-Thinking-Q4_K_M.gguf
D:\models\Ternary-Bonsai-2-27B\Ternary-Bonsai-2-27B-PTQ1_0.gguf
D:\runtimes\llama.cpp\b11756-cuda12.4\llama-server.exe
D:\runtimes\llama.cpp-prism\prism-b10787-cuda12.4\llama-server.exe
D:\runtimes\opencode\1.19.3\opencode.exe
D:\cargo-target\harness                                   (build output)
```

Verify the weights against the checksums in [models.md](models.md) after download.

## Build the `agent` binary

1. Open a shell with the Visual Studio environment loaded (`vcvars64.bat`).
2. In the repository, run:

   ```bat
   set CARGO_TARGET_DIR=D:\cargo-target\harness
   cargo build -p harness-cli --bin agent
   ```

3. The binary is `D:\cargo-target\harness\debug\agent.exe`.

If cargo cannot reach crates.io (for example because the git config sets a proxy), build offline from vendored
crates. The development machine uses the vendor set `D:\cargo-vendor\harness-new` and 4 build jobs
(`CARGO_BUILD_JOBS=4`); see [troubleshooting.md](troubleshooting.md#cargo-cannot-download-crates-on-windows).

## Write the machine config

The machine config records where models, runtimes and OpenCode live on this computer. Write it once:

```bat
D:\cargo-target\harness\debug\agent.exe init --machine --models-dir D:\models --runtimes-dir D:\runtimes --opencode-bin D:\runtimes\opencode\1.19.3\opencode.exe
```

See [configuration.md](configuration.md) for what the machine config may set.

## Set up a project

1. Go to the repository you want to work in.
2. Create the project config and generated OpenCode files, validate them, start the `main` model, check its
   health, and run a task:

   ```bat
   cd /d path\to\your\repo
   D:\cargo-target\harness\debug\agent.exe init --profile local-only
   D:\cargo-target\harness\debug\agent.exe config validate
   D:\cargo-target\harness\debug\agent.exe models serve main --detach
   D:\cargo-target\harness\debug\agent.exe health main
   D:\runtimes\opencode\1.19.3\opencode.exe run "your task here"
   ```

3. For the interactive interface, run `opencode` without `run`.

`agent init` writes a `.gitignore` entry for `.local-agent/`.

## Clear proxy variables

If your network sets `HTTP_PROXY` or `HTTPS_PROXY`, the OpenCode plugin loader and model catalog fetch go through
the proxy and stall. Run OpenCode with those variables cleared for this project (see
[troubleshooting.md](troubleshooting.md#opencode-hangs-at-startup-or-debug-config-never-returns)). The harness's
own localhost calls already ignore the proxy.

## Check the installation

```bat
D:\cargo-target\harness\debug\agent.exe doctor
```

Expect warnings for anything not set up yet. `doctor` must not panic.

## Limits

- Release 1 runs on native Windows only. WSL support moves to release 2.
- Only one machine has been verified (see the status line).
- The `micro` and `deep` slots are used only when their weights and runtimes are present and configured.

## Related

- [configuration.md](configuration.md): machine and project config, profiles, keys and validation rules.
- [models.md](models.md): weights, checksums, runtimes and measured resource use.
- [troubleshooting.md](troubleshooting.md): problems seen on the target machine and their fixes.
- [../concepts/architecture.md](../concepts/architecture.md): how the harness fits into OpenCode.
- [../reference/modules/cli-commands.md](../reference/modules/cli-commands.md): reference for the `agent` commands.
