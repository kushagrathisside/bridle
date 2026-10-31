# How decisions are made

Every tool call the agent makes passes through Bridle before it runs. Bridle classifies the call, checks it against
the settings, and returns one of three answers: allow, block, or refuse. The decision and its reason are recorded. This
page explains the steps, so you can predict what Bridle will do and read why it did it.

## The three answers

| Answer | What happens |
|---|---|
| Allow | The call runs. |
| Block | The call does not run. The agent gets the reason, and the rule that fired is named in the message. |
| Refuse | The call does not run, because a loop rule fired. It applies only when the task workflow is on. See [tasks and loops](tasks-and-loops.md). |

A call that asks for permission is never allowed in an unattended run. Bridle treats "ask" as a denial, because a run
with no one to answer would otherwise approve it. This is the main difference from running the agent on its own. See
[introduction](../overview/introduction.md#compared-with-the-alternatives).

## What Bridle looks at

For each call, Bridle works out:

1. **The tool.** A file read, a file edit, a shell command, or one of Bridle's own tools. Read-only calls are never
   refused by a loop rule.
2. **The class.** Every shell command gets one class, from least to most dangerous:

   | Class | Default answer | Examples |
   |---|---|---|
   | `READ_ONLY` | Allow | `git status`, reading a file |
   | `WRITE` | Allow | editing a file inside the repository |
   | `EXECUTE_SAFE` | Allow | `python -m unittest`, `cargo test` |
   | `EXECUTE_RISKY` | Block | a program that is not on the safe list, `make`, `git -c ...` |
   | `NETWORK` | Block | `curl`, `git push`, web fetches |
   | `DESTRUCTIVE` | Block | `rm -rf`, `git reset --hard`, force pushes |
   | `PRIVILEGED` | Block | `sudo`, system services, registry writes |

   The answer for `EXECUTE_RISKY`, `NETWORK` and `DESTRUCTIVE` comes from the settings `security.risky_commands`,
   `security.network` and `security.destructive_commands`. Their default is `ask`, and `ask` blocks. The answer for
   `PRIVILEGED` comes from `security.privileged_commands`, whose default is `deny`.
3. **The paths.** Every path must stay inside the repository. Secret files are denied to file tools and to shell
   commands. The control files of the harness itself (`local-agent.toml`, `opencode.json`, `.opencode/`, `.local-agent/`,
   and `.git`) cannot be written by the agent, though it may read them.
4. **The arguments.** A shell command is split into its parts the way the shell that runs it would split it. A command
   that cannot be parsed is blocked.

Bridle takes the most dangerous class of any part of a command. A command that chains a safe command with a risky one
is blocked.

## Examples

These are real answers from the Bridle policy, run through the same hook that OpenCode calls. The file names are
shortened.

Reading a secret file is blocked:

```json
{"action":"block","message":"blocked by policy (READ_ONLY, secret_file): config/.env may contain credentials and is never sent to a model","class":"READ_ONLY","verdict":"deny","rule":"secret_file"}
```

Reading the README is allowed:

```json
{"action":"allow","message":"","class":"READ_ONLY","verdict":"allow","rule":"tool_kind"}
```

A network call is blocked, and the verdict is `ask`, which the agent cannot answer:

```json
{"action":"block","message":"blocked by policy (NETWORK, command_risk): curl contacts the network","class":"NETWORK","verdict":"ask","rule":"command_risk"}
```

Running the project's unit tests is allowed as a safe command:

```json
{"action":"allow","message":"","class":"EXECUTE_SAFE","verdict":"allow","rule":"command_risk"}
```

## What happens after a call runs

Output from a tool is redacted before the model reads it. Known secret shapes are replaced, and assignments to secret-like
names have their values removed. Output from Bridle's own tools is redacted by Bridle itself. Redaction is a control,
and it is not a guarantee. The [security model](security.md) lists what it does not catch.

## Where the record is

- `.local-agent/tool-events.jsonl` has one line per call, with its verdict and rule.
- With the task workflow on, each call and each refusal is also in the task journal, which `agent trace` and
  `agent explain` print. See [run a task](../guide/run-a-task.md).

## When a decision is wrong

A block is a rule working as written. If a command you need is blocked, read the rule in the message:

- A command that is not built in and is not on the safe list is blocked as `EXECUTE_RISKY`. Add its program name to
  `security.safe_commands` in the machine configuration. See [run your tests](../guide/run-verification.md).
- A path outside the repository, or a secret file, stays blocked. Move the work into the repository, or read the file
  yourself.
