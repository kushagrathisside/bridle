# The desktop app (`bridle`)

`bridle` opens a window on a folder. It runs the same agent loop, tool policy and event log as `bridle tui`; see
[ADR-0017](../decisions/0017-desktop-app-over-the-agent-loop.md).

```bat
cd path\to\your\project
bridle
```

To get the `bridle` command, build once and install it for your user (it copies the exe to `%LOCALAPPDATA%\Bridle\bin` and adds
that folder to your user PATH; open a new terminal afterwards):

```bat
cargo build --release -p bridle-desktop
scripts\install-bridle.cmd target\release\bridle.exe
```

`bridle FOLDER` opens a given folder, and `bridle --config FILE` adds an explicit configuration file. On the first run in a
folder it asks before writing `bridle.toml` (the `local-only` profile) and the `.gitignore` entries. On the first run on
a computer it asks where the model weights and runtimes are. It starts the default model by itself and stops the servers it
started when it closes.

## The window

| Area | What it does |
|---|---|
| Activity bar | Files, Search, Changes, Models, Info. A second click hides the sidebar. |
| Status bar | Git branch, the model slot and its state, tokens used, the time of the last refresh, and for a file the line, column and language. |
| Editor | Monaco with tabs and breadcrumbs. Ctrl+S saves. A file the agent changes updates by itself unless you have unsaved edits; then the tab is marked `!` and saving asks what to do. |
| Diff tabs | The working copy against the last commit, side by side. Read only. |
| Terminal | Your own shell (Ctrl+`). Pick Git Bash, PowerShell, Command Prompt or WSL. What you type here is not checked by the model's policy. |
| Chat | Streams the answer, shows each tool call as a card with its verdict, and has Open file and View diff buttons for file tools. |

Shortcuts: Ctrl+P go to file, Ctrl+Shift+F search, Ctrl+Shift+E explorer, Ctrl+Shift+G changes, Ctrl+S save, Ctrl+W close tab,
Ctrl+J chat, Ctrl+` terminal, Ctrl+L new chat, Ctrl+O change project, Esc stops a running turn. While the terminal has focus its
keys belong to the shell.

## The team, the board and ideas

The crew rail on the right edge shows the team; the board button at the top right opens the board as a popup, with the saved ideas as
its first column. Tag a member in the chat with `@name`. See [the team, the board and the ideas queue](team-and-board.md).

## The `/` menu and images

Type `/` in the chat for the commands (`/compact`, `/clear`, `/image`, `/team`, `/model`) and for the tools the model can use;
picking a tool fills in "Use the ... tool to". **Images:** the Image button, `/image`, pasting, or dropping a file attaches a
PNG, JPEG, GIF or WebP up to 4 MiB. It is copied to `.bridle/attachments` and sent with your next message. The slot must
have a vision projector (`launch.mmproj_file`); otherwise the window says so before sending.

**Compaction.** When the history passes its budget the oldest turns are folded into a summary; `/compact` does it for the
whole conversation. The meter next to Send shows tokens used out of the slot's window.

## Network

The Info view has a Network card for how Bridle reaches the internet (cloud models, web tools, downloads). Local model servers
never use a proxy. Choose **the system proxy** (the environment's `HTTPS_PROXY` and friends, the default), **a proxy** you
type (`http://host:port`, `https://host:port` or `socks5://host:port`; no password in the address), or **no proxy**. **Test**
sends one request to the address you give, using the values in the form, before you save. The setting lives in the machine
config (`[network] proxy_mode`, `proxy`); a repository cannot set it. An authenticated proxy goes in the environment variable
`BRIDLE_PROXY`, which overrides the setting and is never shown.

## Settings that matter

| Setting | Default | Meaning |
|---|---|---|
| `models.main.context` | 65,536 | Window of the main model. 98,304 passes the VRAM guard but leaves 754 MiB free on an 8 GB GPU. |
| `models.main.launch.mmproj_file` | `Qwen3.5-9B/mmproj-F16.gguf` | The vision projector, relative to `paths.models_dir`. |
| `network.proxy_mode` | `environment` | `environment`, `custom` (with `network.proxy`) or `none`. Machine config only. |
| `models.main.launch.mmproj_offload` | `false` | `false` keeps the projector in system memory (saves about 700 MiB of VRAM; image turns are a little slower). |

## Limits

Windows only. Not a release build; there is no installer. The VRAM guard is an estimate (weights counted at 85 % of the file,
a fixed 736 MiB for buffers, calibrated on one model and one GPU) and refuses a start that is known not to fit. A project
switch stops the model and loads it again in the new folder (about 10 s).
