# Run your tests

Bridle runs your test, lint and type-check commands only when they are declared in the project configuration. Each
declared command is a verification step. The agent calls the verification tools to run them, and Bridle checks each
command before it runs.

This page assumes the repository is set up. See [set up a repository](set-up-a-repository.md).

## Declare a step

Add one `[[verification.steps]]` table per command to `local-agent.toml`:

```toml
[[verification.steps]]
name = "unit tests"
command = "python -m unittest -q"
timeout_ms = 120000
```

| Key | Required | Default | Meaning |
|---|---|---|---|
| `command` | yes | none | The command line to run. |
| `name` | no | none | A label shown in the results. |
| `timeout_ms` | no | 600000 (10 minutes) | How long the step may run. The process tree is stopped when it expires. |
| `cwd` | no | the repository root | A folder relative to the repository root. |
| `kind` | no | `test` | One of `test`, `lint`, `typecheck`, `build`, `other`. |
| `targets` | no | `false` | When `true`, the test tool may add test files, node ids or test names to the command. |

A command that is one of the built-in safe commands runs without any extra entry. A command that is not built in is
blocked unless it is listed in the machine configuration (see below).

## Choose which tool runs which steps

| Tool | Runs |
|---|---|
| `run_verification` | Every step, in order. |
| `run_tests` | The steps of kind `test`. |
| `run_linter` | The steps of kind `lint`. |
| `run_typechecker` | The steps of kind `typecheck`. |

A step of kind `build` runs only through `run_verification`.

A step that fails is information, not an error: the tool reports `failed` with the exit code, the last part of the
output, and the failing tests that Bridle could parse from that output. A step that Bridle blocks reports `blocked` and
does not run.

## Built-in safe commands

These commands run without an entry in the configuration. The agent's shell still sees them as ordinary commands,
and each one is checked by the same policy.

- `pytest`
- `python -m pytest`, `python -m unittest`, `python -m mypy`, `python -m ruff`, and `python -m black --check`
- `cargo test`, `cargo build`, `cargo check`, `cargo clippy`, `cargo fmt`, `cargo doc`, `cargo tree`, `cargo metadata`
- `go test`, `go build`, `go vet`
- `cmake --build`
- `ruff check`, and `ruff format --check`
- `ctest`, `mypy`, `pyright`, `tsc`, `eslint`, `nvidia-smi`

## Run a command that is not built in

For a command such as `make test`, add its program name to `security.safe_commands` in the machine configuration, or in a
file you pass with `--config`. A project file cannot do this, because a cloned repository could then mark its own
scripts as safe. A listed prefix only upgrades a command that would otherwise be classed as risky; anything more
dangerous in the same command line is still blocked.

## Let the agent rerun one test

Set `targets = true` on a test step. Then `run_tests` can narrow the run to given files, node ids or test names:

```toml
[[verification.steps]]
name = "pytest"
command = "python -m pytest -q"
targets = true
```

Targets are checked before use. They may contain only letters, digits and `_ . / \ : - = , @ +`. They may not start
with `-`, contain `..`, or be absolute paths. A target that names a secret file is refused.

## Check the result

```bat
python -m unittest -q
```

The sample in [the worked example](../examples/fix-a-failing-test.md) shows a failing run and a passing run of the same
command.

## Known limits

- A configuration setting named `required_for_done` exists, and the validator warns about it, but no code path reads it
  yet. Bridle does not block a task from finishing because a verification step has not passed.
