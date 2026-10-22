# ADR-0003: tree-sitter for deterministic code intelligence

| | |
|---|---|
| **Status** | Accepted |
| **Date** | 2026-10-22 |
| **Decision owner** | Project lead (brief §14, §15: deterministic retrieval first) |
| **Supersedes** | — |

## Context

M2 needs `repo_map`, `file_summary`, `symbol_search` and `find_references`. The brief says retrieval must be
deterministic first (ripgrep, AST, Tree-sitter, LSP, Git, symbol index) and that vector search is not the first
solution (R-15.1, R-15.2). Text search alone cannot tell a definition from a use, or a real identifier from one
inside a comment or a string.

## Decision

Use tree-sitter, with three grammars for release 1: Rust, Python and C++ (`.c` and `.h` files also use the C++
grammar, which parses plain C well enough for symbol lookup). The grammars are pinned exactly in the workspace:

| Crate | Version | Licence |
|---|---|---|
| `tree-sitter` | 0.27.1 (needs Rust 1.90; the project pins 1.95) | MIT |
| `tree-sitter-rust` | 0.24.2 | MIT |
| `tree-sitter-python` | 0.25.0 | MIT |
| `tree-sitter-cpp` | 0.23.4 | MIT |

No persistent index is built in M2. Files are parsed on demand, and only after a cheap text prefilter. A cached
symbol index belongs to M3 (state).

## Consequences

- Syntactic, not semantic: `find_references` matches identifiers with the same spelling. It does not resolve
  overloads, imports or types. The tool result says so. LSP-grade answers would need a language server, which
  is out of scope for release 1.
- Each grammar compiles C code through `cc`. On Windows this needs the MSVC build environment, which the
  Windows build scripts already load.
- Adding a language later is one grammar crate and one arm in `Language::from_path`.

## Alternatives considered

- **ripgrep only:** fast, but cannot separate definitions from uses or skip comments and strings.
- **A language server (LSP):** semantic, but needs a running per-language server and a project build. Too heavy
  for the first version; a later adapter can add it.
- **A vector index:** the brief rules it out for the first version (R-15.1).
