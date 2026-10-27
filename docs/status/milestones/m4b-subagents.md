# M4b: the subagent tier

The brief planned subagents: a small model reads a bounded set of inputs and returns evidence, never an edit (R-12.2,
R-12.3, R-13.1, R-13.2, R-18.3). Until this milestone only the micro model decided stalled loops (M4), so the tier was
not built. The checklist exposed that gap. This page records what was built, where it is plugged in, and what the live
run showed.

> **Status:** Built and plugged into the MCP tool list and the CLI (2026-10-27). Quality measured on a set of nine
> questions, three runs each, against the micro model (LFM2.5-1.2B on the CPU): 24 of 27 answers passed. The one
> question that failed did so in all three runs. The sample is small; a difference of a few runs is not a finding.

## What was built

- **Core** (`harness-core/src/subagent.rs`): the two roles, `explore` (answer a task from the inputs, citing them) and
  `summarise` (condense one text to its failures). The bounds: inputs are cut to `max_input_bytes`, and at most
  `max_inputs` are kept. The checked answer: a summary, up to 8 findings with an optional reference, up to 5 unresolved
  items, a confidence from 0 to 1, a recommendation. The check refuses anything outside that contract. A reference must
  name an input the call gave (`path` or `path:line`); any other reference is dropped and counted.
- **Runtime** (`harness-runtime/src/subagent.rs`): `gather_inputs` reads each path through the tool policy, refuses a path
  the policy refuses (naming the rule), and redacts each file before anything leaves the function. `answer` makes one
  model call with the role's system prompt and the bounded inputs, then checks the reply. `run_for_config` is the one entry
  point for both the tool and the command, so they cannot drift.
- **Tool**: `subagent` (`harness-runtime/src/mcp/tools/subagent.rs`), offered to the model only when
  `[subagents] enabled = true`. Its answer is labelled as a reading, not a fact.
- **Command**: `agent subagent --role explore --task "..." --path file`, the same path, for manual checks.
- **Config**: `[subagents]` with `enabled` (false by default), `slot` (`micro`), `max_input_bytes` (4096) and `max_inputs`
  (4). Validation V37 (the slot must be defined) and V38 (the limits are in range).

## Plug-in points

| Entry point | Where | Test |
|---|---|---|
| MCP tool list | `crates/harness-cli/src/commands/mcp.rs` (`SubagentTool`, when enabled) | `the_subagent_tool_is_offered_only_when_the_tier_is_on` |
| CLI command | `crates/harness-cli/src/main.rs` (`Command::Subagent`) | `the_command_refuses_when_the_tier_is_off` |
| Config | `crates/harness-core/src/config/schema.rs` (`SubagentsSection`) | `v37_subagents_need_a_defined_slot`, `v38_subagent_limits_are_bounded` |

## Live check (2026-10-27)

Model: LFM2.5-1.2B-Thinking, Q4_K_M, CPU, slot `micro`. Input: `calc.py` with `def add(a, b): return a - b`. Question:
"What does add(a, b) return? Cite the line."

| Run | Conclusion | Reference handling |
|---|---|---|
| 1 | correct (`a - b`) | 1 reference named no input and was dropped |
| 2 | correct | 1 dropped |
| 3 | correct | none dropped |
| 4 | correct | 1 dropped |
| 5 | unclear ("purpose and output are unclear") | none dropped |

Four of five conclusions were correct. The citation check did what it should on every run: references it could not verify
were dropped and counted in the text the parent reads.

Two changes were tried and reverted, and the reasons are recorded here:

1. **Numbered input lines** (`   2: return a - b`). The model read the numbers as citations, dropped them, and then said the
   return value was not in the input. Reverted.
2. **Bare line numbers accepted for a single input** (`2`, `line:2`, `line 2` become `calc.py:2`). Kept. It is a rule, not a
   guess: with one input, a bare number can only mean that input. It is covered by a unit test.

## Quality over the eval set (2026-10-27)

`evals/subagent/run.py` runs `agent subagent` for each case in `evals/subagent/cases.json`, three times, and scores the
answer body (the summary, the findings and the recommendation) against the case's expected patterns. The raw result
is in `evals/subagent/results/2026-10-27-micro.json`.

| Case | Passed |
|---|---|
| What `add` returns (explore `calc.py`) | 3 of 3 |
| What `mul` returns | 3 of 3 |
| Whether a `divide` function exists (answer no) | 3 of 3 |
| `RETRY_LIMIT` value | 3 of 3 |
| Cache directory | 3 of 3 |
| Which service reported the ERROR line | 0 of 3 |
| Failed HTTP status | 3 of 3 |
| Which order was retried | 3 of 3 |
| Summary of the errors (summarise role) | 3 of 3 |

Total: 24 of 27 runs passed; 0 unavailable; 0.89 over answered runs.

The failing case: the model said "an error related to payment status" and did not name the service `payments`. It also
said the error details were not in the input, which they were. The answer was the same in all three runs, so it is a
consistent miss, not noise. Six runs had a reference dropped by the citation check.

The first run of the script reported all 27 runs as unavailable. The cause was in the batch file, not the model: an
`echo` whose value ran into `>>` was read as a redirect of standard input, so the exit line never reached the file. The
script now writes the line with a parenthesised echo, and it counts a missing answer header as unavailable.

## Limits

- The answer quality of a 1.2B model is limited. The tier is useful for narrowing a search, not for conclusions. The
  parent is told so in every answer.
- Nine questions, three runs each, is still a small sample. The set is in `evals/subagent/cases.json`; it covers two
  roles, three fixture kinds and one absence question (a function that does not exist). It does not cover long inputs
  near the byte limit, or inputs that mix a secret with an answer. The citation check counts a line number that is past
  the end of the file as valid: it checks the file name only (the answer is not verified against the line).
- Confidence is the model's own estimate. It is not calibrated.
- The tool reads only what the policy allows, and only up to the configured limits. A repository cannot enable it: the
  setting is read from the machine config through the trust rules.

## Related

- [ADR-0009: subagents are bounded readers](../../decisions/0009-subagents-are-bounded-readers.md)
- [Checklist](../checklist.md): rows M4b-1 to M4b-8
