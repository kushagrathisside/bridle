# Tasks and loops

A task is one objective the agent works on in a repository. Bridle records each call made towards the task in a
journal. Loop rules read the journal and refuse a call when the agent is going round in circles, so the agent changes
its approach or asks for a decision instead of repeating itself.

The task workflow is off in the default configuration. Turn it on as described in [run a task](../guide/run-a-task.md).
Nothing on this page applies while it is off.

## Phases

A task moves through phases. Bridle infers each phase from the calls it observes; the model does not declare them.

Start, Discover, Plan, Implement, Verify, Review, Done, Fail, Diagnose, Explore, and Escalate.

For example, an edit moves the task to Implement, a test run moves it to Verify, and a failing test moves it to Fail and
then Diagnose. A passing test moves it to Review.

## The loop rules

Each rule has a threshold. The defaults are in the table, and each one can be changed in the project's `[workflow]`
or `[agent]` section, as shown in the [configuration reference](../guide/configuration.md).

| Rule | Fires when | Default | What the agent is told |
|---|---|---|---|
| `repeated_command` | The same command runs again and again with no edit in between | 3 times within the last 5 calls | Read its output, edit a file, then run the command again. |
| `repeated_edit` | One file is edited again and again since the last verification | 4 edits | Run the verification before editing the file again. |
| `same_failure` | The same failure comes back in consecutive verifications | 3 in a row | Read the failure and change the approach, or ask the user. |
| `implementation_retries` | The task enters Implement more often than allowed | more than 3 times | Explain the blocker to the user and wait for a decision. |
| `iteration_limit` | The task reaches its iteration limit | 8 iterations | The task is blocked. Run `agent task reset` to continue. |

When more than one rule fires, the first one in this order decides: iteration limit, implementation retries, same
failure, repeated edit, repeated command.

## What a refusal looks like

The agent sees the refusal as the result of its call. A repeated-edit refusal is this message:

```text
refused by loop rule repeated_edit: this file was edited again and again since the last verification.
Allowed next: run the verification before editing the file again.
```

The fifth edit to the same file in a row was refused in a test on the tested machine, after four edits had been allowed.
The record for that refusal reads:

```text
call e5: AskUser because RepeatedEdit (confidence 1.00, provider rules), refused
  signals: RepeatedEdit { file: "calc.py", count: 4 }
```

## How refusals end

A refusal stops the call, not the task. Three things end a block:

- The agent makes progress that the rules can see. An edit clears a repeated-command count, and a passing verification
  clears the failure and edit counts.
- You run `agent task reset`, which starts the counters again. The journal is kept.
- The refusal cap is reached. After three refusals in a row, Bridle stops refusing on loop grounds, so that a session
  cannot be locked up. The policy still blocks the calls it blocks for other reasons.

## What is not covered

- Read-only calls are never refused by a loop rule, so the agent can always read files to understand a failure.
- Loop rules are heuristics. They can refuse a deliberate retry. The message names the rule, so you can decide.
- The rules see only the calls Bridle sees: its own tools, and shell test runs. A test run is judged from its output,
  never from an assumption that it passed.
