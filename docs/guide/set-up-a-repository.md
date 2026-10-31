# Set up a repository

This page adds Bridle to a repository. It covers the command that does it, the files it writes, the two profiles, and
how to check the configuration. You need the `agent` binary first; see [install Bridle](installation.md).

## Run the setup

From the repository folder:

```bat
agent init --profile local-only
agent config validate
```

`init` writes four things and reports each one:

```text
✓ wrote local-agent.toml (profile local-only)
✓ wrote opencode.json
✓ wrote .opencode/plugins/local-agent.ts
✓ .gitignore: added .local-agent/, .worktrees/
```

| File | What it is |
|---|---|
| `local-agent.toml` | The project configuration. Read it, and commit it if the team should share it. |
| `opencode.json` | The OpenCode configuration that points OpenCode at Bridle's model servers and tools. Generated; do not edit it by hand. |
| `.opencode/plugins/local-agent.ts` | The plugin that OpenCode loads. It sends every tool call to Bridle's policy. Generated. |
| `.gitignore` entries | `.local-agent/` holds the run records and logs. `.worktrees/` holds task worktrees. |

If you edit a generated file by hand, `init` does not replace it. To replace it, run `agent init --force`, which also
overwrites the edits.

`init` never overwrites an existing `local-agent.toml`. Use `--workspace <dir>` to set up a repository that is not the
current folder.

## Choose a profile

`init` takes `--profile local-only` (the default) or `--profile development`.

| Profile | Use it for | Difference from the other |
|---|---|---|
| `local-only` | Everyday use. No external services are enabled. | The default. |
| `development` | Working on Bridle itself or debugging the policy. | Telemetry keeps fields up to 16384 bytes instead of 2048, and the micro model's reasoning budget is 4096 instead of 8192. |

The other profiles in the repository (`hybrid`, `local-with-jev`, `benchmark`, `test`) turn on external services or are
for tests. They are not covered here; see [status and limits](../project/status.md).

## Check the configuration

```bat
agent config validate
```

A valid configuration prints one line and exits with code 0:

```text
✓ configuration is valid (2 file(s))
```

Warnings also exit with code 0. Errors exit with code 1 and name the rule and the key. A repository with no
verification steps gets one warning, `V15`, until you add a test command. [Run your tests](run-verification.md) shows how.

To see every loaded file and the settings that apply:

```bat
agent config paths
agent config show
```

`config paths` lists the machine configuration and the project configuration it loaded. `config show` prints the merged
result. It redacts values under keys with secret-like names. On the tested machine `secrets_redaction = true` appeared
as `[REDACTED]`. That display is wrong, and the setting is still on; the known issue is listed in
[status and limits](../project/status.md).

## What the project file may set

The project file travels with the repository, so Bridle reads it with trust rules. A project file may tune the
workflow, verification steps and most behaviour. It may not loosen security or choose executables and endpoints. The
full list is in the [configuration reference](configuration.md).

## Next steps

- [Run your tests](run-verification.md), so the agent can run them through Bridle.
- [Serve the models](models.md), so the agent has a model to talk to.
- [Run a task](run-a-task.md), to start tracking a task and read its decisions.
