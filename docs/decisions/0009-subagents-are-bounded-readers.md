# ADR-0009: Subagents are bounded readers, checked before the parent sees them

- **Status:** Accepted (2026-10-27, with the M4b result)
- **Date:** 2026-10-27
- **Deciders:** Kushagra Srivastava

## Context

The brief plans subagents: a small model that reads part of the work and returns evidence (R-12.2, R-12.3). It returns a
structured result (R-13.1) and the parent never gets the whole transcript (R-13.2). The small micro model is the first
candidate (R-5.3). Until now the tier was not built.

## Decision

1. A subagent reads and answers. It never edits, never runs a command, and never sees the parent's transcript. Its only
   inputs are the task text, up to 4 files named by the parent, and a literal text.
2. Each input goes through the tool policy first. A refused path is an error that names the rule. Each input is redacted
   before it leaves the process, then cut to a fixed size.
3. The answer is checked against a fixed schema. A reply that is not that shape is an error, not an answer. A reference
   must name one of the inputs; any other reference is dropped and counted. A bare line number is accepted only when there
   is one input.
4. The parent receives a rendering labelled as a reading, with every loss said (dropped references, cut findings, inputs
   left out). It is bounded to 2000 bytes.
5. The tier is off by default. It is one config switch, and the tool is offered only when the switch is on.

## Consequences

- A subagent answer can be wrong. The label and the citation check reduce the risk; they do not remove it. The parent must
  check cited lines before it relies on an answer.
- The cost of a subagent call is one model call on the small model, with a bounded prompt. Nothing about the main model's
  context grows beyond the answer.
- The tier is measured on a small sample only. A larger quality measurement is open (checklist row M4b-8).

## Alternatives considered

- A subagent that edits a copy of the repository and proposes a patch: rejected for now. It needs a diff review step, and
  the brief keeps the small models away from implementation (R-4.3).
- Numbered input lines to help citations: tried and reverted. The small model misread the numbers.

## References

- [M4b result](../status/milestones/m4b-subagents.md)
- [Checklist](../status/checklist.md)
