# Window screenshots

Bridle can start a program, wait for its window to appear, take a picture of **that window only**, and then stop the program. The facility
is intended for checking what a program that you have built looks like. It is **off by default**, available on **Windows only**, and is a
command like any other: the program is judged by the same rules as `run_command`. The design, and what it deliberately does not do, are
described in
[ADR-0021](../decisions/0021-window-screenshots-start-the-program-and-capture-only-its-window.md).

## Enabling it

Enable it in the **machine** configuration (`%APPDATA%\bridle\config.toml`); a repository's `bridle.toml` cannot set it:

```toml
[desktop]
screenshots_enabled = true
max_wait_ms = 30000          # the longest a call may wait for the window to appear (1000 to 120000)

[security]
safe_commands = ["myapp"]    # the program must be permitted to run, as for any other command
```

`bridle config validate` checks the limits (rule V50). Nothing needs to be installed: the capture uses the PowerShell that is supplied with
Windows.

## From the terminal

```text
bridle desktop screenshot winver
bridle desktop screenshot python app.py --wait-ms 15000
bridle desktop screenshot myapp --title "Settings"
```

The program must be a bare name that can be found on `PATH`, and its arguments may contain only letters, digits and the characters
`_ . / - = : , @ +`. If the policy would not allow `run_command` to start the program, it is not started here either, and the answer names
the rule that refused it.

## What the model receives

The tool `window_screenshot`, which is offered only when the facility is enabled, accepts `program`, `args`, `cwd` (inside the workspace),
`wait_ms` and `title_contains`. It answers in text of the following form, in which the values are placeholders:

```text
Window screenshot [ID] of `PROGRAM`: WIDTH x HEIGHT pixels, title "TITLE", drawn by printwindow; about N distinct colours. The window has content.
Saved as .bridle/web/screenshots/NAME.png (sha256 ...). `PROGRAM` was started by Bridle and stopped after the picture.
```

- **`drawn by`** states the method. `printwindow` means that the window drew itself, which also works for windows that use the graphics
  processor; `screen` means that the corresponding part of the screen was copied, which requires the window to be visible and uncovered.
- **BLANK** means that the picture is a single colour throughout: the window may not yet have been drawn, or it cannot be captured. A longer
  `wait_ms` is the usual remedy.
- The picture is recorded as a source (`kind: "screenshot"`, `extraction: "window"`) in `.bridle/web/sources.jsonl`, together with its SHA-256
  value. A model that can interpret pictures sees it only if you attach the file.

## What it does not do

- It does not click, type or move the mouse, and it does not look at the desktop or at the window of any other program.
- It always stops the whole process tree that it started, whatever the outcome.
- A window of more than sixteen million pixels is refused, and a picture of more than 8 MiB is not kept.
- It is not offered on other operating systems, and a repository cannot enable it.

## Points to note

- **A picture may show personal data.** The About window of Windows, for example, shows your user name. The picture remains in `.bridle/`
  in the project, which Git ignores, and you decide whether to attach or share it.
- The facility has been tried on one program (`winver`) on one display. A window that cannot render itself and is covered by another window
  may be captured incorrectly; the blank check detects a single colour, but not a window that is only partly drawn.
- To test the facility yourself, set `BRIDLE_WINDOW_TESTS=1` and run the window test, which opens a real window.

## Related

- [The managed browser](web-browser.md) takes pictures of web pages with `browser_snapshot`.
- [Configuration](configuration.md)
