# End-to-end suite for the window (Windows, no WSL)

`python tools\e2e\run.py --exe target\debug\bridle.exe` launches the app on a throwaway project, runs the named checks in
`suite.json` against the real window, closes it, and verifies that no `bridle.exe` or `llama-server.exe` is left. Add
`--skip-model` to skip the checks that load the model (no GPU use). `cdp.py` is the bounded client: a hard watchdog, read
timeouts and size caps, so it cannot hang or grow (see the notes of 2026-11-03 in docs/status/progress.md).
