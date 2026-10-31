# Install Bridle

This page takes a Windows machine from nothing to a built `agent` binary, and explains the machine configuration that
tells Bridle where your models and OpenCode are. Setting up a repository comes next, in
[set up a repository](set-up-a-repository.md).

Bridle is verified on native Windows only. The steps below were run on Windows 11 with an NVIDIA RTX 4060 Laptop GPU.

## Prerequisites

| Item | Version used on the tested machine | Notes |
|---|---|---|
| Windows | 11, build 26200 | Native Windows. WSL is not supported in this release. |
| Visual Studio 2022 | Community, with the C++ workload | Provides the MSVC linker. Build from a developer shell, or load `vcvars64.bat` first. |
| Rust | 1.95.0, MSVC host | Pinned in `rust-toolchain.toml`. `rustup` installs it on first build. |
| Git | Any recent version | Needed to clone the repository and for worktree tasks. |
| OpenCode | 1.19.3 | Portable Windows build. Bridle is wired into this exact version. |
| llama.cpp | b11756, CUDA 12.4 | Runs the `main` and `micro` model servers. Add the CUDA runtime archive from the same release. |
| llama.cpp fork | `prism-b10787-73bfe6c`, CUDA 12.4 | Only for the optional `deep` model. |
| NVIDIA driver | 592.82 or later with CUDA 12.4 support | `nvidia-smi` must work from a terminal. |
| Python | 3.13 | Only if you want to run the sample project's tests. |

Model files are not included. See [serve the models](models.md) for the files, their sources and checksums.

## 1. Get the source

```bat
git clone https://github.com/kushagrathisside/bridle.git
cd bridle
```

## 2. Build the `agent` binary

Open a developer shell, or run `"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"`
first, so the linker is on the path. Then:

```bat
cargo build -p harness-cli --bin agent
```

Cargo writes the binary to `target\debug\agent.exe`. To keep build output outside the repository, set `CARGO_TARGET_DIR`
before you build, and use the same folder when you run the binary.

Check the build:

```bat
target\debug\agent.exe --version
```

```text
agent 0.1.0
```

The examples in these docs write `agent` for the binary. Add its folder to your `PATH`, or call it by its full path.

## 3. Write the machine configuration

The machine configuration records where the model files, the inference runtimes and OpenCode live on this computer. You
write it once:

```bat
agent init --machine --models-dir <models-dir> --runtimes-dir <runtimes-dir> --opencode-bin <path-to-opencode.exe>
```

Replace the three placeholders with your folders. A layout that works is one folder for the weights, one for the runtimes
(`llama.cpp\b11756-cuda12.4\llama-server.exe`, and `opencode\1.19.3\opencode.exe`), and the repository's own build folder.

The file is written to `%APPDATA%\local-agent\config.toml`. Check where it is and what it holds:

```bat
agent config paths
```

The repository's own configuration cannot change these folders or the executables, so a cloned repository cannot point
Bridle at another OpenCode.

## 4. Check the installation

```bat
agent doctor
```

`doctor` checks the host, the runtimes, the model files, the endpoints, the OpenCode version and the configuration. On a
machine where the model servers are not running yet, it reports the endpoints as unreachable. Those are warnings, not
failures. It exits with code 1 only when a configured runtime is missing or the configuration has errors.

A healthy first run looks like this, abbreviated:

```text
✓ gpu: NVIDIA GeForce RTX 4060 Laptop GPU | VRAM 8.0 GiB total
✓ models dir: <path> 101.2 GiB free
✓ runtime llama_cpp (llama_cpp): <path> build 11756
✓ opencode: <path> | 1.19.3
⚠ endpoint main: health: endpoint unreachable
✓ summary: no blocking problems (4 warning(s))
```

## Limits

- This release runs on native Windows. The WSL runtime is not built.
- The configuration and the policy work without the model files. The agent needs them to run a task.
- Bridle needs the exact OpenCode version it was built against. A different version may not load the plugin.

## Next steps

- [Set up a repository](set-up-a-repository.md).
- [Serve the models](models.md).
