# ADR-0021: A window screenshot starts the program itself and captures only that program's window

- **Status:** Accepted (2026-11-04); built and run once against a real program
- **Date:** 2026-11-04
- **Deciders:** Kushagra Srivastava

## Context

The owner asked: *does Bridle have the ability to take a screenshot of a window by launching it, and how could it be done?* An agent that
builds a program with a window (a desktop tool, a small game, a web view) has no way to know what the window looks like. Until now Bridle
could run a command and read its text, and the managed browser could take a picture of a page, but nothing could show a native window.

The answer was no. This ADR is how it was built, and what it deliberately does not do.

## Decisions

### 1. It is a command first

`window_screenshot` starts a program, so it is judged exactly like `run_command`: the program must be a bare name on `PATH`, the
arguments must be plain words, and the same shell policy, command class and sandbox apply (`mcp::tools::run::authorize`, extracted so
both tools call one function). A program the policy would not run is not started here either; the answer says which rule refused it.
The tool is in `EXECUTING_HARNESS_TOOLS`, so an executing tool cannot be reached by a look-alike name.

To use a program you build, add it to `security.safe_commands` in the machine configuration, as for any command.

### 2. It is off by default and machine-config only

`[desktop] screenshots_enabled = false` and `max_wait_ms = 30000` (validation rule V50: between 1,000 and 120,000). A repository
cannot turn it on: `desktop` is one of the sections `config::trust` refuses in a project file, with the same test that covers `web`,
`network` and `fleet`. The tool is offered only on Windows and only when enabled.

### 3. Only the launched program's windows are looked at

A PowerShell script built into Bridle (`desktop/capture-window.ps1`, passed with `-EncodedCommand`; its settings are environment
variables, never text pasted into the script) lists the top-level windows owned by the **process tree** of the program Bridle started,
and nothing else. It picks the largest visible one (or the one whose title contains `title_contains`), waits up to `wait_ms` for it to
appear, lets it finish painting, and takes the picture with `PrintWindow(PW_RENDERFULLCONTENT)`, which also asks windows that draw with
the GPU (WebView2, Electron, DirectX) for their content. If the result is a single colour all over, it falls back to copying that part
of the screen (which needs the window to be on screen and uncovered), and the answer says which method drew the picture.

It does not click, type, move the mouse, or look at the desktop or at any other program's window. A minimised window is restored first.
Windows larger than 16 million pixels are refused; the picture is kept only if it is a PNG of at most 8 MiB.

There is no new Rust dependency and no `unsafe` code: the Windows calls are inside the script.

### 4. What was started is always stopped

After the picture (or the failure), Bridle ends the whole process tree it started (`kill_tree`) whatever happened. A program that exits
by itself before its window appears is reported as such.

### 5. The picture is evidence, not a message to the model

The PNG is stored by hash in `.bridle/web/screenshots/` (ignored by Git) and recorded in `sources.jsonl` as `kind: "screenshot"`,
`extraction: "window"`, with its SHA-256, like every other artifact. The model's answer is text only: the size, the title, how it was
drawn, about how many colours there are, and a plain verdict, **blank** or **has content**, from a 64 by 64 sample of the picture. A model
that can look at pictures sees it only when the person attaches the file; Bridle does not push images into a text-only model. The tool
log records which program ran and how long it took, not the window's title or pixels.

## Alternatives considered

| Option | Why not |
|---|---|
| Capture the whole screen or the desktop | Sees every other window: mail, a password manager, a chat. The agent asked for its own program's window |
| Computer-use style control (screenshots plus clicks and keys) | An authority independent of the policy; not what was asked, and a much larger surface |
| A Rust Win32 binding or a UI-automation crate | A new dependency and `unsafe` code under `unsafe_code = deny`, for a feature that is one script |
| Let the model write the capture script | The script is the boundary; it is fixed and built in, and nothing the model sends is placed in it |

## Consequences

- **Verified.** On this machine `winver` was launched, its window found, a correct picture taken with `printwindow`, the program ended.
  Unit tests cover: the tool is offered only when enabled and only on Windows; arguments are checked like `run_command` before anything
  starts; a program the policy would not run is not started; the script travels as an encoded command and never as a file; a picture
  of one colour is called blank. An opt-in test that opens a real window runs with `BRIDLE_WINDOW_TESTS=1`.
- **Personal data.** A window can show a user name, a path or a document; `winver` showed the Windows user's name. The picture stays in
  `.bridle/` on this machine; the person decides whether to attach or share it.
- **Not verified.** Only one program was tried. Windows that cannot print themselves and are covered by another window will give a
  wrong or blank picture with the screen-copy method; the blank verdict is the signal, and a longer `wait_ms` or a visible window
  is the remedy. High-DPI scaling relies on the script marking itself DPI-aware, which was seen working on one display only.
- **A second use.** `bridle desktop screenshot PROGRAM [ARGS...]` runs the same tool, with the same checks, for a person at the terminal.

## Related

- [Window screenshots (guide)](../guide/window-screenshots.md)
- [ADR-0020: the managed browser](0020-managed-browser-is-a-policy-gated-playwright-worker.md)
