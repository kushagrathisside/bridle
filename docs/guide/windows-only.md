# Working on Windows without WSL

Everything Bridle needs to be built, tested and run is native Windows. This page lists what used to go through WSL and the
Windows route that replaced it, each checked on the tested machine (2026-11-03).

| Used to need WSL | Windows route | Checked |
|---|---|---|
| Refreshing vendored dependencies (cargo timed out reaching crates.io) | `scripts\dev.cmd vendor http://172.31.2.4:8080`: native `cargo vendor` through the proxy | Same 500 crates, 499 of 500 identical checksums |
| Building, linting, testing | `scripts\dev.cmd gate`, `build`, `release` | 921 tests; clippy clean |
| Driving the window for tests (a PowerShell client hung and caused the 2026-11-03 crash) | `scripts\dev.cmd e2e`: `tools/e2e`, stdlib Python with a watchdog | 14 of 14 checks, nothing left running |
| Downloading a model file | `curl.exe -L -o FILE URL` (Windows 10 and 11 ship it), then `certutil -hashfile FILE SHA256` | The projector was fetched this way once; checksum compared |
| Vendoring Monaco and xterm.js | Download the npm tarball with `curl.exe` from `registry.npmjs.org/<package>/-/<package>-<version>.tgz` and unpack it with `tar -xzf` | Not re-run; the files in `ui/vendor` are the result |
| A Linux shell in the editor | Git Bash, native, in the terminal's shell picker | Ran commands |
| The coding assistant's own session | Start it on `D:\src\harnessforkush`, not a WSL folder; `CLAUDE.md` carries the rules | Memory copied to the Windows project folder |

## Why the network step needs a proxy

This network lets traffic out only through `http://172.31.2.4:8080`. `curl.exe` works both directly and through it, but cargo's
direct connections and the app's direct requests time out. Set the proxy for the app in the window (Info, Network), or in the
machine config; for cargo use `CARGO_HTTP_PROXY`. See [the desktop guide](desktop.md#network).

## Shutting WSL down

When nothing is started from WSL any more: `wsl --shutdown` in a Windows terminal frees its memory (its `.wslconfig` allows it
11 GB; its idle services use about 1 GB). Docker Desktop also uses WSL, so quit it too. The old WSL clone
`/home/kushagra/harnessforkush` is stale (frozen at M2) and can be deleted.
