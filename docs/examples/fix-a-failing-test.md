# Fix a failing test

The sample project has one bug: `add()` in `calc.py` subtracts instead of adding. Its unit test fails until the bug is
fixed. This example walks through an agent run that fixes it, with Bridle deciding every tool call.

It needs the `agent` binary, the model server, and OpenCode. Complete [getting started](../overview/getting-started.md)
first.

## 1. Copy the sample project

Copy the folder out of the Bridle repository, so the agent edits a copy:

```bat
xcopy path\to\bridle\examples\toy-python-repo my-toy\ /E /I
cd my-toy
git init
git add .
git commit -m "Initial sample"
```

Run the test to see the failure:

```bat
python -m unittest -q
```

This is the output from the sample on the tested machine, with Python 3.13. The paths are shortened:

```text
======================================================================
FAIL: test_add (test_calc.CalcTests.test_add)
----------------------------------------------------------------------
Traceback (most recent call last):
  File "test_calc.py", line 8, in test_add
    self.assertEqual(add(2, 3), 5)
    ~~~~~~~~~~~~~~~~^^^^^^^^^^^^^^
AssertionError: -1 != 5

----------------------------------------------------------------------
Ran 2 tests in 0.000s

FAILED (failures=1)
```

## 2. Set up Bridle for the project

```bat
agent init --profile local-only
```

Tell Bridle how to run the tests. Open `local-agent.toml` and add a verification step at the end:

```toml
[[verification.steps]]
name = "unit tests"
command = "python -m unittest -q"
timeout_ms = 120000
```

`python -m unittest` is one of the commands Bridle knows to be safe, so it runs without a configuration entry. Check the
file:

```bat
agent config validate
```

The `V15` warning from getting started is gone:

```text
✓ configuration is valid (2 file(s))
```

## 3. Start the model and run the agent

```bat
agent models serve main --detach
agent health main
opencode run "Fix the bug in calc.py so the unit tests pass."
```

While the agent works, each tool call goes through the policy. A `read` of a file that looks like a secret is denied. A
shell command that is not on the safe list is blocked, and the agent is told why. Edits are allowed. The agent can run
the tests with the `run_tests` tool, which runs the step you added.

## 4. Check the result

```bat
python -m unittest -q
```

A passing run ends with `OK` and exits with code 0. This is the result of the same sample with the one-character fix
applied by hand, on the tested machine:

```text
----------------------------------------------------------------------
Ran 2 tests in 0.000s

OK
```

Then look at what Bridle recorded for the task:

```bat
agent task status
```

`task status` prints the phase and the counters. The verdict for every call is in `.local-agent/tool-events.jsonl`. The
loop decisions, with their reasons, are shown by `agent explain` once the task workflow is enabled; see
[run a task](../guide/run-a-task.md).

## Notes

- The model's changes differ from run to run. This page records the test output, not a model run. The model used on
  the tested machine is listed in [models](../guide/models.md).
- If the agent blocks a command you expected it to run, read the message. It names the rule. Safe commands are listed
  in [the configuration reference](../guide/configuration.md#run-your-tests).
