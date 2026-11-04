# Window screenshots

Bridle can start a program, wait for its window, take a picture of **that window only**, and stop the program. It is for checking what a
program you built looks like. It is **off by default**, **Windows only**, and a command like any other: the program is judged by the same
rules as `run_command`. The design and what it does not do are in
[ADR-0021](../decisions/0021-window-screenshots-start-the-program-and-capture-only-its-window.md).

## Turn it on

In the **machine** configuration (`%APPDATA%\bridle\config.toml`; a repository's `bridle.toml` cannot set it):

```toml
[desktop]
screenshots_enabled = true
max_wait_ms = 30000          # the most a call may wait for the window to appear (1000 to 120000)

[security]
safe_commands = ["myapp"]    # the program must be allowed to run, as for any command
```

`bridle config validate` checks the limits (rule V50). Nothing needs installing: the capture uses the PowerShell that comes with Windows.

## From the terminal

```text
bridle desktop screenshot winver
bridle desktop screenshot python app.py --wait-ms 15000
bridle desktop screenshot myapp --title "Settings"
```

The program must be a bare name found on `PATH`; its arguments may use letters, digits and `_ . / - = : , @ +` only. If the policy would
not let `run_command` start it, it is not started here, and the answer names the rule.

## What the model gets

The tool `window_screenshot` (offered only when enabled) takes `program`, `args`, `cwd` (inside the workspace), `wait_ms` and
`title_contains`. It answers in text, in this shape (the values here are placeholders):

```text
Window screenshot [ID] of `PROGRAM`: WIDTH x HEIGHT pixels, title "TITLE", drawn by printwindow; about N distinct colours. The window has content.
Saved as .bridle/web/screenshots/NAME.png (sha256 ...). `PROGRAM` was started by Bridle and stopped after the picture.
```

- **`drawn by`** is `printwindow` (the window drew itself, which also works for windows drawn by the GPU) or `screen` (a copy of that part
  of the screen; the window must be visible and not covered).
- **BLANK** means one colour all over: the window may not have drawn yet, or cannot be captured. Try a longer `wait_ms`.
- The picture is a source record (`kind: "screenshot"`, `extraction: "window"`) in `.bridle/web/sources.jsonl` with its SHA-256. A model
  that can look at pictures sees it only if you attach the file.

## What it will not do

- It does not click, type or move the mouse, and it does not look at the desktop or at any other program's window.
- It stops the whole process tree it started, whatever happened.
- A window over 16 million pixels is refused, and a picture over 8 MiB is not kept.
- It is not offered on other systems, and a repository cannot turn it on.

## Things to know

- **A picture can show personal data** (the About box of Windows shows your user name). It stays in `.bridle/` in the project, which Git
  ignores; you decide whether to attach or share it.
- Tried on one program (`winver`) on one display. Windows that cannot print themselves and are covered by another window can come back
  wrong; the blank check catches a single colour, not a half-drawn window.
- To test the real thing yourself: `set BRIDLE_WINDOW_TESTS=1` and run the window test, which opens a real window.

## Related

- [The managed browser](web-browser.md) takes pictures of pages with `browser_snapshot`.
- [Configuration](configuration.md)
