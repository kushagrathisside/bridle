# M2: deterministic tools

Status: in progress (branch `m2-tools`, started 2026-10-22). Steps 0 to 6 done; step 7 (artifacts and experiments) next. Release 1 targets Windows, so every step is built
and tested on Windows (`D:\cargo-target\wgate.bat`).

## Goal

The brief (§14, §15, §21) asks for deterministic tools before any LLM judgement: "Do not use an LLM to perform a
task that can be answered deterministically" (R-14.1). M2 delivers the initial tool suite as MCP tools, with the
path guard on every tool, so Qwen can locate code, run checks and read their failures without guessing.

## Scope

| Requirement | Delivered by |
|---|---|
| R-14.2 initial tools | the tools below |
| R-14.3 research tool adapters (API) | declarative `[[tools.adapters]]` |
| R-14.4 nothing repository-specific in the core | adapters live in config |
| R-15.1, R-15.2 deterministic retrieval, no vector database | Git, text search, tree-sitter syntax trees |
| R-21.2, R-21.5 verification by kind | `run_tests`, `run_linter`, `run_typechecker` over declared steps |
| R-22.2 structured results | every tool returns structured content |
| R-0.3 Python, C++, Rust | grammars and failure parsers for the three |
| AC-A5 failures are parsed | `parse_test_failure`, applied to failed steps |

## Tools

| Tool | What it returns | Class |
|---|---|---|
| `git_status`, `git_diff` | (M1) | READ_ONLY |
| `git_history` | commits touching a path (`--follow`), bounded | READ_ONLY |
| `git_log_range` | commits and changed files between two revisions | READ_ONLY |
| `repo_map` | tracked files by directory, languages, key files; secret files left out | READ_ONLY |
| `file_summary` | size, lines, language, top-level symbols with lines, imports | READ_ONLY |
| `symbol_search` | definitions by name (exact, prefix, substring), kind and language | READ_ONLY |
| `find_references` | identifier uses (not in comments or strings), with line text | READ_ONLY |
| `dependency_search` | declared dependencies (Cargo, pyproject, requirements, CMake, package.json) and the files that import one | READ_ONLY |
| `parse_test_failure` | failures from test, compiler or linter output: name, file, line, message | READ_ONLY |
| `inspect_config` | the effective config, redacted, with its files and validation issues | READ_ONLY |
| `inspect_artifact` | a JSON, JSONL, CSV or log file summarised: keys, columns, numeric stats, tail | READ_ONLY |
| `summarize_experiment`, `compare_experiments` | config and metrics of a run directory; metric differences between two | READ_ONLY |
| `run_verification` | (M1) every declared step | steps checked inside |
| `run_tests`, `run_linter`, `run_typechecker` | declared steps of that kind; tests may take policy-checked targets | steps checked inside |
| `run_command` | one program with arguments, no shell, after the full policy check | checked inside |
| `adapter_<name>` | a configured research or project tool | checked inside |

## Rules every tool follows

- Every path argument and every file a tool reads passes the read policy (workspace containment, secret files).
- Output is bounded (counts, bytes, time) and redacted by the server before it leaves.
- Commands run without a shell where possible; when a shell is needed, the command is classified in that shell's
  grammar.
- No persistent index in M2: files are read on demand from `git ls-files`, with a text prefilter before parsing.
  A cached symbol index belongs to M3 (state).

## Steps (each a reviewable commit, Windows gate green)

0. Split `mcp/tools.rs` into modules; shared path and process helpers.
1. `git_history`, `git_log_range`.
2. Verification kinds; `run_tests`, `run_linter`, `run_typechecker`.
3. `parse_test_failure`; failed steps return parsed failures.
4. `run_command`, `inspect_config`.
5. Tree-sitter (ADR-0003): `file_summary`, `symbol_search`, `find_references`, `repo_map`.
6. `dependency_search`.
7. `inspect_artifact`, `summarize_experiment`, `compare_experiments`.
8. Adapters; per-tool enable and disable (each tool's schema costs prompt tokens on a 16K-context model).
9. Hook and policy updates, docs, traceability, end-to-end with Qwen, tag `m2`.
