# ADR-0017: A desktop window over the same agent loop, with Tauri

- **Status:** Accepted (2026-11-03)
- **Date:** 2026-11-03
- **Deciders:** Kushagra Srivastava

## Context

The product so far is a terminal tool (`bridle`) with OpenCode as the interactive host. On the tested machine OpenCode's
window opens blank ([troubleshooting](../guide/troubleshooting.md)). The goal is an application like Claude Code or Codex,
started with one command in a folder, on Windows.

## Decision

1. **A second front end, not a privileged one.** The window runs `chat::Agent`: the same policy, loop rules, redaction and
   event log as the OpenCode hook and `bridle tui`. `harness-cli` became a library plus the `bridle` binary; the new crate
   `bridle-desktop` (binary `bridle`) calls it. The window has no file, shell or network access of its own except through
   its commands (`capabilities/default.json`).
2. **Tauri 2 with a plain HTML, CSS and JavaScript front end.** It uses the WebView2 that Windows 11 ships. No Node build
   step: Monaco (editor) and xterm.js (terminal) are vendored under `ui/vendor` with their licences. The page's content
   security policy allows scripts, workers and styles from the app only.
3. **What the person does is not what the model does.** The editor, the explorer, search and the terminal act for the
   person and are not checked by the model's policy. They stay inside the folder, refuse `.git` and `.bridle`, write
   atomically, refuse a file that changed on disk unless told to overwrite, and "delete" by moving to `.bridle/trash`.
4. **The terminal is a real pseudo-terminal** (ConPTY through `portable-pty`) with Git Bash first. A native bash needs no
   virtual machine; WSL is offered and starts its VM only while it is open.
5. **A start of a GPU model must fit** ([`harness_runtime::vram`](../reference/modules/README.md)): at least 768 MiB must stay
   free after the load.

## Alternatives

- **egui (pure Rust).** Fewer moving parts, but weaker diff and markdown rendering. Rejected for the editor experience.
- **OpenCode's own web interface.** Owns the loop, so the harness could not show its decisions or compaction. Rejected.
- **A full Tauri bundle with Node tooling.** More polish, more build steps on a machine that cannot reach crates.io.

## Consequences

- Dependencies reach Windows only through `D:\cargo-vendor\bridle-desktop`, vendored from WSL. See the WSL audit.
- The binary has the whole agent loop in it; `bridle` stays the scripting interface.
- Not built here: a packaged installer, signing, a release (GUI-subsystem) build run, and macOS or Linux.
