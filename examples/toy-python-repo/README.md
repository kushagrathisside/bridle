# toy-python-repo

The M1 exit target. `calc.py` has one bug: `add()` subtracts. The unit tests fail until it is fixed.

Run the tests with `python -m unittest -q`.

To exercise the harness end to end (Windows, release 1):

1. Copy this folder to a new location and `git init` it.
2. `agent init --profile local-only` in that folder. Add a verification step to `local-agent.toml`
   (`python -m unittest` is a built-in safe command, so no `safe_commands` entry is needed):

   ```toml
   [[verification.steps]]
   name = "unit tests"
   command = "python -m unittest -q"
   timeout_ms = 120000
   ```

3. Start the main model with `agent models serve main --detach`.
4. Run `opencode run "Fix the bug in calc.py so the unit tests pass."` in that folder.
