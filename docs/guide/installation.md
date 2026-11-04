# Install Bridle

This page takes a Windows machine from nothing to a working `bridle`, and explains the machine configuration that tells Bridle where your
models and runtimes are. There are two ways in: the **release zip** (no build tools) and a **build from source**. Setting up a
repository comes next, in [set up a repository](set-up-a-repository.md).

Bridle is verified on native Windows only. The steps below were run on Windows 11 with an NVIDIA RTX 4060 Laptop GPU.

## What you need

| Item | Version used on the tested machine | Needed for |
|---|---|---|
| Windows | 11, build 26200 | Everything. Native Windows only; WSL is not supported in this release. The window uses WebView2, which ships with Windows 11. |
| NVIDIA GPU and driver | 592.82 or later with CUDA 12.4 support | The local models. `nvidia-smi` must work from a terminal. The shipped profile is tuned for 8 GB of video memory. |
| Model files | See [serve the models](models.md) | The agent. **Not included**; the `main` model is required, the others are optional. |
| llama.cpp | b11756, CUDA 12.4 (plus the CUDA runtime archive of the same release) | Runs the model servers. |
| llama.cpp fork | `prism-b10787-73bfe6c`, CUDA 12.4 | Only for the optional `deep` model. |
| Git for Windows | Any recent version | Recommended: Git Bash is the terminal's default shell, and checkpoints, lanes and worktree tasks use `git`. |
| OpenCode | 1.19.3, portable Windows build | Only if you want to drive Bridle from OpenCode. Bridle is wired into this exact version. |
| Visual Studio 2022, Rust, Python 3 | Community with the C++ workload; Rust 1.95.0 (pinned in `rust-toolchain.toml`); Python 3.13 | Only to build from source (and Python to run the sample project's tests). |

## Install from the release zip

1. Unpack `bridle-<version>-windows-x64.zip` anywhere.
2. Check the download and compare with `SHA256SUMS.txt`:

   ```bat
   certutil -hashfile bridle.exe SHA256
   ```

3. Run `install.cmd`. It copies `bridle.exe` to `%LOCALAPPDATA%\Bridle\bin` and adds that folder to your **user** `PATH`. It changes nothing
   else on the machine.
4. Open a **new** terminal (so the new path is read) and check:

   ```bat
   bridle --version
   ```

   ```text
   bridle 0.1.0
   ```

The program is not code-signed, so Windows SmartScreen may warn the first time. To uninstall, delete `%LOCALAPPDATA%\Bridle\bin` and remove
that folder from your user `PATH`.

## Build from source

```bat
git clone https://github.com/kushagrathisside/bridle.git
cd bridle
```

Open a developer shell, or run `"C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Auxiliary\Build\vcvars64.bat"` first, so the
linker is on the path. Then use the task runner:

```bat
scripts\dev.cmd build        :: debug build of bridle and bridle-cli
scripts\dev.cmd release      :: release build of bridle.exe
scripts\dev.cmd install      :: release build, then install for this user
```

`scripts\dev.cmd` builds offline from vendored dependencies and sets `CARGO_BUILD_JOBS=4`; run one build at a time. Without it, the plain
command is `cargo build --release -p bridle-desktop`, which writes `target\release\bridle.exe`. The headless command-line program is
`cargo build -p harness-cli`, which writes `target\debug\bridle-cli.exe`. To keep build output outside the repository, set
`CARGO_TARGET_DIR` before you build. See [contributing](../../CONTRIBUTING.md) for the full developer setup, and
[releasing](../project/releasing.md) for how a release zip is made.

The examples in these docs write `bridle` for the program.

## Write the machine configuration

The machine configuration records where the model files, the inference runtimes and OpenCode live on this computer. The window asks for
the folders the first time it starts. From the terminal, you write it once:

```bat
bridle init --machine --models-dir <models-dir> --runtimes-dir <runtimes-dir> --opencode-bin <path-to-opencode.exe>
```

Replace the placeholders with your folders. A layout that works is one folder for the weights, one for the runtimes
(`llama.cpp\b11756-cuda12.4\llama-server.exe`, and `opencode\1.19.3\opencode.exe`), and the repository's own build folder. The file is
written to `%APPDATA%\bridle\config.toml`. Check where it is and what it holds:

```bat
bridle config paths
```

A repository's own configuration cannot change these folders or the executables, so a cloned repository cannot point Bridle at another
program.

## Check the installation

```bat
bridle doctor
```

`doctor` checks the host, the runtimes, the model files, the endpoints, the OpenCode version and the configuration. On a machine where the
model servers are not running yet, it reports the endpoints as unreachable. Those are warnings, not failures. It exits with code 1 only when
a configured runtime is missing or the configuration has errors. A healthy first run looks like this, abbreviated:

```text
✓ gpu: NVIDIA GeForce RTX 4060 Laptop GPU | VRAM 8.0 GiB total
✓ models dir: <path> 101.2 GiB free
✓ runtime llama_cpp (llama_cpp): <path> build 11756
✓ opencode: <path> | 1.19.3
⚠ endpoint main: health: endpoint unreachable
✓ summary: no blocking problems (4 warning(s))
```

`bridle doctor --repair` first puts right what a crash left behind (see [runs, checkpoints and recovery](../reference/run-recovery.md)).

## Limits

- This release runs on native Windows. The WSL runtime is not built.
- The configuration and the policy work without the model files. The agent needs them to run a task.
- If you use OpenCode, Bridle needs the exact version it was built against. A different version may not load the plugin.

## Next steps

- [Set up a repository](set-up-a-repository.md).
- [Serve the models](models.md).
