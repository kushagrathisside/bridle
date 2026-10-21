# Troubleshooting

Each entry is a problem seen on this machine, with what fixed it.

## OpenCode hangs at startup, or `debug config` never returns

Cause: OpenCode fetches its model catalog from models.dev, and the plugin loader runs `bun` for plugin
dependencies. Both go through the proxy in `HTTP_PROXY`, and both stall. A plugin-free run (`opencode --pure`) returns
in about a second, so the stall is in those network calls, not in the plugin.
Fix: run OpenCode with `HTTP_PROXY`, `HTTPS_PROXY` and their lower-case forms cleared. Example:

```bat
set HTTP_PROXY=
set HTTPS_PROXY=
set http_proxy=
set https_proxy=
D:\runtimes\opencode\1.19.3\opencode.exe run "task"
```

## `serve` says the server did not become healthy, but the log shows it listening

Cause: the health probe went through the environment proxy. Fixed in the harness: the loopback client never uses a proxy.
If you see this on an old build, rebuild.

## `serve` says "already has a recorded server"

Cause: a PID record exists from an earlier run. Run `agent models stop <slot>` first. It removes stale records, so it
is safe after a crash or a manual kill.

## `health` shows "warming" or 503

Cause: the model is still loading. Wait for `serve` to finish, or run `health` again. A 503 is not an error.

## `health` says "speculative decoding active" for a model that is not using it

Cause: fixed. The check now treats `speculative.types = "none"` as off.

## A tool call is blocked with "unrecognised program"

Cause: the command is not in the classifier's list, so the default is deny (decision D3). If it is your project's own
command, add it to `security.safe_commands` (see [configuration.md](configuration.md)).

## A Windows path is blocked as "alternate data stream" or "path outside workspace"

Cause: earlier builds lexed `D:\dir` with POSIX rules, which broke the path. Fixed: Windows commands use the `cmd`
lexer. Rebuild if you see it.

## `cargo` cannot download crates on Windows

Cause: the Windows git config (`%USERPROFILE%\.gitconfig`) sets an `http.proxy`, which cargo follows. Work around
it by vendoring from a machine that can reach crates.io (`cargo vendor`) and building offline with
`--offline --config "source.crates-io.replace-with='vendored-sources'"`. Do not commit proxy credentials.

## `.env` does not show up in `git status`

Cause: the global gitignore (`%USERPROFILE%\.gitignore_global`) ignores `.env`. This is expected.

## The model does not start: "llama-server exits during start-up"

Check the log at `<repo>\.local-agent\logs\<slot>.log`. Common causes: a missing model file (check `D:\models`), or a
machine config that points at the wrong directory (`agent config show`).

## `agent doctor` shows "relative and paths.runtimes_dir is not set"

Cause: the machine config was not written. Run `agent init --machine` with the directories.
