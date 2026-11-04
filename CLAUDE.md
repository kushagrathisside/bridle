# Bridle: notes for an AI coding session

Bridle is a local-first coding agent: a Rust harness (policy, tools, redaction, event log) with a desktop window (Tauri),
a terminal UI, and small local models run through llama.cpp. **This repository is worked on natively on Windows. Do not use
WSL.** The Linux build lives on a separate machine and is not touched here.

## Layout

- `crates/harness-core`: config, security policy, command classifier, redaction, identity (no network).
- `crates/harness-runtime`: model providers, HTTP routes (proxy), server launcher, VRAM check, MCP server and tools.
- `crates/harness-cli`: the commands, the chat agent loop, the TUI; a library plus the headless `bridle-cli`.
- `crates/bridle-desktop`: the `bridle` program (window plus the same commands). The UI is plain JS in `ui/`; Monaco and
  xterm.js are vendored under `ui/vendor`.
- `tools/e2e`: the end-to-end suite for the window. `scripts/dev.cmd`: the task runner.
- `docs/`: the pages are partly stale; code and `docs/status/progress.md` are the truth. Decisions are in `docs/decisions`.

## Commands (Windows, from the repository root)

```bat
scripts\dev.cmd gate           :: format check, clippy -D warnings, all tests
scripts\dev.cmd e2e            :: builds, then drives the real window (needs the GPU model; --skip-model avoids it)
scripts\dev.cmd release        :: release build of bridle.exe
```

Dependencies are vendored (`D:\cargo-vendor\bridle-desktop`), so builds are offline. This network only lets traffic out through
the proxy `http://172.31.2.4:8080`: `scripts\dev.cmd vendor http://172.31.2.4:8080` refreshes them natively. One build at a
time, `CARGO_BUILD_JOBS=4` (the machine has 16 GB of RAM and an 8 GB GPU).

## Rules that were learned the hard way

- **Names:** the product is Bridle (`bridle.toml`, `.bridle/`, `BRIDLE_*`). The old `local-agent` names are read as a
  fallback and never written (ADR-0018). Do not introduce them again.
- **No runaway helpers.** A hung PowerShell debug script once leaked 32 GB and killed the machine's session. Every helper
  script needs a wall-clock limit, read timeouts and size caps; use `tools/e2e/cdp.py`, not PowerShell, to drive the window.
- **Stop model servers with `bridle models stop <slot>`**, never a task killer, and check `nvidia-smi` is at 0 MiB after a
  test. A GPU model must leave at least 768 MiB of VRAM free; the guard is in `harness_runtime::vram`.
- **Code style:** `unsafe` is forbidden; clippy runs with `-D warnings`; no `unwrap` outside tests. Match the surrounding
  code's comment density and naming.
- **Commits and pull requests:** do not add "Claude", "Anthropic", AI, co-author or "reviewed" wording. Commit only when asked.
- **Network:** requests to remote endpoints go through the route in `network.proxy_mode` (machine config, set in the window's
  Info view). Local model servers never use a proxy.
