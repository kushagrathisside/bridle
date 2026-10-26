# M4b: the subagent tier

The brief planned subagents: a small model reads a bounded set of inputs and returns evidence, never an edit (R-12.2,
R-12.3, R-13.1, R-13.2, R-18.3). Until this milestone only the micro model decided stalled loops (M4), so the tier was
not built. The checklist exposed that gap. This page records what was built, where it is plugged in, and what the live
run showed.

> **Status:** Built and plugged into the MCP tool list and the CLI (2026-10-27). Live check run against the micro model
> (LFM2.5-1.2B on the CPU): the answer was correct in 4 of 5 repeated runs. Quality measured on one question only.

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

## Limits

- The answer quality of a 1.2B model is limited. The tier is useful for narrowing a search, not for conclusions. The
  parent is told so in every answer.
- Five repeats on one question is a small sample. A quality measurement on a larger set is open (checklist row M4b-8).
- Confidence is the model's own estimate. It is not calibrated.
- The tool reads only what the policy allows, and only up to the configured limits. A repository cannot enable it: the
  setting is read from the machine config through the trust rules.

## Related

- [ADR-0009: subagents are bounded readers](../../decisions/0009-subagents-are-bounded-readers.md)
- [Checklist](../checklist.md): rows M4b-1 to M4b-8
