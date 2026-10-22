# Installation (Windows, release 1)

Status: verified on one machine (Windows 11, RTX 4060 Laptop 8 GB, 16 GB RAM). Other setups are not tested.

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

The Bonsai deep model needs the PrismML fork (`prism-b10787-73bfe6c`, CUDA 12.4). It is not needed for release 1 (M5).

## Layout

```
D:\models\Qwen3.5-9B\Qwen3.5-9B-Q4_K_M.gguf
D:\models\LFM2.5-1.2B-Thinking\LFM2.5-1.2B-Thinking-Q4_K_M.gguf          (M4)
D:\models\Ternary-Bonsai-2-27B\Ternary-Bonsai-2-27B-PTQ1_0.gguf          (M5)
D:\runtimes\llama.cpp\b11756-cuda12.4\llama-server.exe
D:\runtimes\llama.cpp-prism\prism-b10787-cuda12.4\llama-server.exe      (M5)
D:\runtimes\opencode\1.19.3\opencode.exe
D:\cargo-target\harness                                                  (build output)
```

Checksums of the weights are in [models.md](models.md). Verify them after download.

## Build

From a shell with the Visual Studio environment loaded (`vcvars64.bat`), in the repo:

```bat
set CARGO_TARGET_DIR=D:\cargo-target\harness
cargo build -p harness-cli --bin agent
```

The binary is `D:\cargo-target\harness\debug\agent.exe`.

## Set the machine config

```bat
D:\cargo-target\harness\debug\agent.exe init --machine --models-dir D:\models --runtimes-dir D:\runtimes --opencode-bin D:\runtimes\opencode\1.19.3\opencode.exe
```

## Set up a project

```bat
cd /d path\to\your\repo
D:\cargo-target\harness\debug\agent.exe init --profile local-only
D:\cargo-target\harness\debug\agent.exe config validate
D:\cargo-target\harness\debug\agent.exe models serve main --detach
D:\cargo-target\harness\debug\agent.exe health main
D:\runtimes\opencode\1.19.3\opencode.exe run "your task here"
```

Run `opencode` (without `run`) for the interactive interface. `agent init` writes a `.gitignore` entry for `.local-agent/`.

## Proxies

If your network sets `HTTP_PROXY` or `HTTPS_PROXY`, OpenCode's plugin loader and model catalog fetch go through
the proxy and stall. Run OpenCode with those variables cleared for this project (see
[troubleshooting.md](troubleshooting.md)). The harness's own localhost calls already ignore the proxy.

## Check

```bat
D:\cargo-target\harness\debug\agent.exe doctor
```

Expect warnings for anything not set up yet. It must not panic.
