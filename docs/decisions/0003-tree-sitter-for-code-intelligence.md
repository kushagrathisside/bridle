# ADR-0003: tree-sitter for deterministic code intelligence

- **Status:** Accepted
- **Date:** 2026-10-22
- **Deciders:** Project lead (brief §14 and §15: deterministic retrieval first)
- **Supersedes:** Nothing

## Context

M2 needs the `repo_map`, `file_summary`, `symbol_search` and `find_references` tools. The brief says
retrieval must be deterministic first (ripgrep, AST, Tree-sitter, LSP, Git, symbol index) and that vector
search is not the first solution (R-15.1, R-15.2). Text search alone cannot tell a definition from a use, or a
real identifier from one inside a comment or a string.

## Decision

Use tree-sitter, with three grammars for release 1: Rust, Python and C++. Files ending in `.c` and `.h` also
use the C++ grammar, which parses plain C well enough for symbol lookup. The workspace pins exact versions:

| Crate | Version | Licence |
|---|---|---|
| `tree-sitter` | 0.27.1 (needs Rust 1.90; the project pins 1.95) | MIT |
| `tree-sitter-rust` | 0.24.2 | MIT |
| `tree-sitter-python` | 0.25.0 | MIT |
| `tree-sitter-cpp` | 0.23.4 | MIT |

M2 builds no persistent index. Files are parsed on demand, and only after a cheap text prefilter. A cached
symbol index belongs to M3 (state).

## Consequences

- The analysis is syntactic, not semantic. `find_references` matches identifiers with the same spelling. It
  does not resolve overloads, imports or types, and the tool result says so. Answers at the level of a
  language server would need one, which is out of scope for release 1.
- Each grammar compiles C code through `cc`. On Windows this needs the MSVC build environment, which the
  Windows build scripts already load.
- Adding a language later takes one grammar crate and one arm in `Language::from_path`.

## Alternatives considered

- **ripgrep only.** Fast, but it cannot separate definitions from uses or skip comments and strings.
- **A language server (LSP).** Semantic, but it needs a running server for each language and a project build.
  Too heavy for the first version; a later adapter can add it.
- **A vector index.** The brief rules it out for the first version (R-15.1).

## Related

- [ADR-0001](0001-rust-core-thin-host-adapter.md): the code tools are served over its MCP server.
- [ADR-0002](0002-runtime-os-repo-location-and-deviation.md): the Windows build environment the grammars
  compile in.
- [Progress log](../status/progress.md): the entry "2026-10-22 (later): M2 step 5 done (tree-sitter code
  tools, ADR-0003)".
- [M2 milestone: tools](../status/milestones/m2-tools.md).
- [Requirements traceability register](../reference/requirements-traceability.md): rows R-15.1 and R-15.2.
