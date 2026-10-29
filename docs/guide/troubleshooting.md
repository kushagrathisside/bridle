# Troubleshooting

This page lists problems seen on the release 1 target machine, each with its cause and fix. It is for an engineer
running the Bridle harness with OpenCode on Windows. It assumes the setup in [installation.md](installation.md).

> **Status:** Release 1 (native Windows). Each entry was observed and resolved on the target machine between
> 2026-10-21 and 2026-10-22.

## Start-up and network

### OpenCode hangs at startup, or `debug config` never returns

**Cause:** OpenCode fetches its model catalog from models.dev, and the plugin loader runs `bun` for plugin
dependencies. Both go through the proxy in `HTTP_PROXY`, and both stall. A plugin-free run (`opencode --pure`)
returns in about a second, so the stall is in those network calls, not in the plugin.

**Fix:** run OpenCode with `HTTP_PROXY`, `HTTPS_PROXY` and their lower-case forms cleared:

```bat
set HTTP_PROXY=
set HTTPS_PROXY=
set http_proxy=
set https_proxy=
D:\runtimes\opencode\1.19.3\opencode.exe run "task"
```

### `cargo` cannot download crates on Windows

**Cause:** the Windows git config (`%USERPROFILE%\.gitconfig`) sets an `http.proxy`, which cargo follows.

**Fix:** vendor the crates from a machine that can reach crates.io (`cargo vendor`) and build offline with
`--offline --config "source.crates-io.replace-with='vendored-sources'"`. Do not commit proxy credentials.

## Model server

### `serve` says the server did not become healthy, but the log shows it listening

**Cause:** the health probe went through the environment proxy.

**Fix:** fixed in the harness; the loopback client never uses a proxy. If you see this on an old build, rebuild.

### `serve` says "already has a recorded server"

**Cause:** a PID record exists from an earlier run.

**Fix:** run `agent models stop <slot>` first. It removes stale records, so it is safe after a crash or a manual
kill.

### `health` shows "warming" or 503

**Cause:** the model is still loading. A 503 is not an error.

**Fix:** wait for `serve` to finish, or run `health` again.

### `health` says "speculative decoding active" for a model that is not using it

**Cause:** an earlier build treated `speculative.types = "none"` as active.

**Fix:** fixed; the check now treats `"none"` as off. Rebuild if you see it.

### The model does not start: "llama-server exits during start-up"

**Fix:** check the log at `<repo>\.local-agent\logs\<slot>.log`. Common causes:

- a missing model file (check `D:\models`);
- a machine config that points at the wrong directory (check with `agent config show`).

### `agent doctor` shows "relative and paths.runtimes_dir is not set"

**Cause:** the machine config was not written.

**Fix:** run `agent init --machine` with the directories (see [installation.md](installation.md#write-the-machine-config)).

## Policy

### A tool call is blocked with "unrecognised program"

**Cause:** the command is not in the classifier's list, so the default is deny.

**Fix:** if it is your project's own command, add it to `security.safe_commands` in the machine config or a
`--config` file (see [configuration.md](configuration.md#runners-that-are-not-built-in)). A project config may not
set it.

### A Windows path is blocked as "alternate data stream" or "path outside workspace"

**Cause:** earlier builds lexed `D:\dir` with POSIX rules, which broke the path.

**Fix:** fixed; commands are lexed in the shell OpenCode runs them in. That is PowerShell on Windows unless `SHELL`
names another shell. Git Bash, for example, uses POSIX rules, so write paths as `D:/dir` there. Rebuild if you see
it.

## Git

### `.env` does not show up in `git status`

**Cause:** the global gitignore (`%USERPROFILE%\.gitignore_global`) ignores `.env`. This is expected.

## Related

- [installation.md](installation.md): prerequisites, build and project setup.
- [configuration.md](configuration.md): safe commands, verification steps and validation rules.
- [models.md](models.md): model files, runtimes and resource use.
- [../concepts/security.md](../concepts/security.md): how the policy classifies commands and paths.
