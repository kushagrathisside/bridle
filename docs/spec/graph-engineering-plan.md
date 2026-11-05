# Graph engineering for understanding code: research and plan

Written 2026-11-05. This document answers a request to find out how *graph engineering* can help an agent to understand a code base, to
survey the tools and repositories that large companies and open-source contributors have published, and to plan how Bridle could
implement it. Nothing described under "The plan" has been built.

## Scope and an assumption

The request asked for a plan "in Moodle". Bridle is the project in which the request was made, and its purpose is to help a model to work
in a code base, so this plan is written for **Bridle**: Bridle would build a graph of whatever repository it is opened on. Moodle is a
large PHP learning-management system and is treated in [an appendix](#appendix-applying-the-plan-to-a-php-code-base-such-as-moodle) as an
example of a repository that Bridle would index. If Moodle itself was meant as the product to be changed, the research and the
design principles below still apply, and the appendix is the place to start.

## What is meant by graph engineering here

A code base is already a graph. Files contain classes and functions; functions call one another; modules import modules; classes inherit;
tests exercise code; commits touch files; people own directories. Graph engineering for code means making that structure explicit, storing
it so that it can be queried quickly, and giving the model *bounded* ways to ask questions of it, such as "what calls this?", "what breaks
if I change this?", "which tests cover it?", and "which parts of the system does this concept involve?". The aim is to replace guesswork
from similar-looking text with answers that follow the real connections.

Two kinds of graph must be kept apart, because the evidence about them differs.

1. **A code graph**, built by parsing: nodes and edges come from syntax, from a compiler or from a language server. It is exact where the
   tool understands the language, and it is rebuilt by a program, not by a model.
2. **A concept (knowledge) graph**, built by a model reading text: entities and relations are extracted by an LLM, as in GraphRAG. It
   covers ideas that the syntax does not state, but it is approximate and costs a model call for every piece of text.

## What exists

### Industrial code-intelligence systems (code graphs)

| System | Who | What it is | Licence and state, as found |
|---|---|---|---|
| [SCIP](https://github.com/sourcegraph/scip) | Sourcegraph | A language-agnostic protocol and Protobuf format for code indexes, used for go-to-definition, find-references and find-implementations. [Announced as a replacement for LSIF](https://sourcegraph.com/blog/announcing-scip). Indexers listed in the repository: Java, Scala and Kotlin; TypeScript and JavaScript; Rust (rust-analyzer); C and C++; Ruby; Python; C# and Visual Basic; Dart; PHP. | Apache-2.0. Active |
| [Glean](https://github.com/facebookincubator/glean) | Meta | Collects *facts* about code and serves them through Angle, a declarative logic query language that also defines schemas. Stored in RocksDB. [Incremental indexing](https://engineering.fb.com/2024/12/19/developer-tools/glean-open-source-code-indexing/) is by stacked immutable databases, aiming at cost proportional to the change. Meta uses it for code browsing, search, documentation, dead-code detection, test selection and retrieval for AI assistants. Can ingest SCIP and LSIF | Open source since August 2021. The page gives query latencies (about a millisecond for a simple lookup) and no fleet-scale figures |
| [Kythe](https://kythe.io/) | Google | "A pluggable, (mostly) language-agnostic ecosystem for building tools that work with code", with a graph schema, indexers (the page lists C++ and Java) and extractors | The page states no licence |
| [Stack graphs](https://github.com/github/stack-graphs) | GitHub | Name resolution expressed as graphs, built with tree-sitter and a graph-construction language, used for precise code navigation | The repository was **archived on 9 September 2025** (read-only). Useful as a design reference; not a dependency to adopt |
| [Joern](https://github.com/joernio/joern), [codebadger](https://github.com/lekssays/codebadger) | Open source (Joern: Apache-2.0), one author (codebadger) | A code property graph (syntax, control flow and data flow in one graph; Joern reads source, bytecode and binaries). codebadger is an MCP server that gives an LLM high-level tools (program slicing, taint tracking, data-flow analysis, navigation) instead of asking it to write graph queries. A [paper](https://arxiv.org/abs/2603.24837) accepted at an ICSE 2026 workshop reports three case studies, among them a correct first-attempt patch for CVE-2025-6021 in libxml2 | Case studies only; the paper gives no baselines or metrics |
| [Aider's repository map](https://aider.chat/docs/repomap.html) | Open source | Parses with tree-sitter, links files by the symbols they define and use, ranks the graph with a PageRank-style method, and fits the top of it into a token budget | Aider's page describes files as nodes and dependency links as edges, keeping the most-referenced symbols that fit a token budget (1,000 tokens by default, `--map-tokens`). The ranking method itself is described by third-party write-ups as PageRank; check Aider's source |

### Research systems that give a model a graph (code graphs)

| System | Idea | Reported evidence (the authors' own) |
|---|---|---|
| [CodexGraph](https://arxiv.org/abs/2408.03910) | Index the repository into a graph database (Neo4j) and let the model write graph queries | Evaluated on CrossCodeEval, SWE-bench and EvoCodeBench; the abstract gives no figures. Retrieval depends on the model writing good queries |
| [RepoGraph](https://arxiv.org/abs/2410.14684) | A plug-in that builds a repository-level graph and retrieves the neighbourhood of the relevant lines | Plugged into four SWE-bench systems; the abstract says it "substantially boosts" all of them, with no figures |
| [LocAgent](https://arxiv.org/abs/2503.09089) | A directed heterogeneous graph of files, classes, functions and their imports, calls and inheritance, searched through a fixed set of tools for multi-hop reasoning | Up to 92.7 % file-level localisation accuracy; a fine-tuned 32B model comparable to proprietary models at about 86 % lower cost; 12 % better issue resolution at Pass@10 |
| [Code Graph Model (CGM)](https://arxiv.org/abs/2505.16901) | Feeds the graph into the model's attention and fine-tunes it, with a four-stage graph-retrieval pipeline | 43.00 % on SWE-bench Lite with an open 72B model (the paper's own ranking claims); a later version reports 50.40 % on SWE-bench Verified. Python and Java only |
| [CodeCompass](https://arxiv.org/abs/2602.20048) | An MCP server that exposes dependency graphs; separates *navigation* (finding what the code depends on) from *retrieval* (finding similar text) | 99.4 % task completion on hidden-dependency tasks, against 76.2 % for an agent without it and 78.2 % with BM25 retrieval; 258 trials on one FastAPI repository; **one author, not peer reviewed**; **58 % of trials that could use the graph made no tool call at all** |

### Concept graphs and memory (knowledge graphs)

| System | What it is | Relevance to code |
|---|---|---|
| [Microsoft GraphRAG](https://microsoft.github.io/graphrag/) and [LightRAG](https://github.com/HKUDS/LightRAG) | An LLM extracts an entity graph from text; GraphRAG adds community summaries for questions about a whole corpus | Built and benchmarked for prose. No independent comparison on code question answering was found. For code, structure is already explicit in the syntax, so extracting edges with a model is costly and less exact than parsing |
| [Graphiti](https://github.com/getzep/graphiti) (Zep) | A bi-temporal knowledge graph for agent memory, with hybrid retrieval and an MCP server; needs Neo4j or FalkorDB | Memory of *what changed and when*. Relevant to history and decisions, not to parsing code |
| Community code-graph servers (GitNexus, CodeGraphContext, code-graph-rag-mcp) | tree-sitter graphs behind an MCP server; one optionally uses SCIP indexers for more accurate calls | Evidence of demand and of a common design. Only secondary write-ups were available, so their quality is unverified |

Searches for Kythe's licence and for GitHub's successor to stack graphs returned nothing definitive, and the search for Cognee and Neo4j's
LLM graph builder returned nothing; they are not assessed.

## What the evidence says

1. **Structure helps when the answer is not in the words.** CodeCompass's gain is on tasks whose dependencies share little vocabulary
   with the code that uses them; on tasks where text search suffices, retrieval did as well as the graph. LocAgent's gain is in locating
   the right place to change. A graph is a means of following connections that similarity search cannot see.
2. **How the model reaches the graph matters as much as the graph.** The three styles are model-written queries (CodexGraph), a context
   plug-in (RepoGraph) and a small set of purpose-built tools (LocAgent, codebadger, CodeCompass). The authors of two of them argue that
   fixed tools beat generated queries, which suits small models that write queries badly.
3. **A model with a tool does not necessarily use it.** In CodeCompass 58 % of the trials that had graph access made no call to it. Bridle's own
   measurements show the same tendency in a different form: a 9B model called `command_status` up to 21 times in a row until the tool's
   text told it to wait ([ADR-0023](../decisions/0023-notices-background-commands-and-lanes-from-infinity.md)). **Any graph tool must be
   designed for adoption and measured for it.**
4. **Most of the numbers are the authors' own, on Python repositories, with frontier or large models.** Nothing found tests a 9B local
   model with a code graph. The results are reasons to try the approach, not predictions of the gain.
5. **Accurate references need a resolver.** Bridle's present tools are syntactic: `find_references` matches a name by spelling and does not
   resolve imports, types or overloads. The industrial systems get precision from a compiler or language server (SCIP indexers, Kythe
   extractors). A graph built only from tree-sitter will contain wrong edges wherever names are shared, and must say so.

## What Bridle has now

`harness_runtime::code` parses with tree-sitter (Rust, Python, C and C++) and offers `repo_map`, `file_summary`, `symbol_search` and
`find_references`. Each call parses the files it needs; **there is no persistent index and no edge between symbols**. `dependency_search`
reads package manifests. The checkpoints and the board record history and ownership, but nothing joins them to the code. The limits page
already lists "no persistent symbol index".

## The plan

### Principles

1. **Parse; do not ask a model to extract.** The code graph is built by programs. A model may *summarise* a node later, and such text is
   labelled as a model's reading, as sub-agent answers already are.
2. **Every edge says where it came from and how sure it is.** An edge carries its source (`syntax`, `scip`, `git`, `model`) and a
   confidence. A name-matched call is `syntax, unresolved`; a call resolved by a SCIP index is `scip`. Tool answers show it.
3. **Bounded answers.** A graph tool returns a small neighbourhood (an ego network, a path, a ranked list) within a token budget that fits a
   small context, and says what it left out.
4. **Read-only, inside the policy.** Graph tools are READ_ONLY harness tools; the index lives under `.bridle/`; secret files are not
   indexed; every answer passes redaction.
5. **Local and incremental.** No service, no cloud. A file is re-indexed only when its hash changes.
6. **Measured before it is believed.** Each phase has an evaluation in `bridle eval-agent` with a baseline and an exit criterion.

### Phase 0: a baseline (small)

Add graph-shaped tasks to the evaluation: *hidden-dependency* tasks (the change must also touch a file that shares no words with the
task), *impact* tasks (which files are affected), and *test-finding* tasks. Run them with today's tools (`repo_map`, `symbol_search`,
`find_references`) on Qwen3.5-9B and record the pass rates and the number of tool calls. **Exit criterion:** a written baseline over at
least ten runs per task type. This is the figure every later phase must beat, and it may show that the tools already suffice for some
tasks.

### Phase 1: a persistent fact index (medium)

Index each source file into facts: files, modules, classes, functions and methods with their ranges; `contains`, `imports`, `inherits`
edges; and *syntactic* `calls` and `references` edges, each marked unresolved. Store them in a compact file under `.bridle/index/`,
keyed by file hash, so a changed file replaces its facts only (the stacked, incremental idea of Glean, without its machinery).
Languages: the three that are parsed now. **Storage decision:** the project forbids unvetted new dependencies and builds offline from a
vendored set. The first choice is a purpose-built, append-only record file with an in-memory adjacency index, in the style of the board's
log; an embedded database such as SQLite is the alternative if the vendored set can take it. A separate graph database server is ruled
out by the local-first principle. **Exit criterion:** indexing the Bridle repository and one larger public repository within a stated time and
memory, and an incremental update touching only changed files.

### Phase 2: resolved references from SCIP (medium)

Read SCIP indexes (Protobuf, Apache-2.0) produced by the existing indexers (`rust-analyzer`, `scip-python`, `scip-clang`, `scip-php`,
`scip-typescript` and others) and replace the unresolved edges with resolved ones where an index is present. Bridle does not run an
indexer by itself: running a language's tooling executes the project's code or build, so it is a command that **the person** runs (`bridle
graph index --scip FILE`), or a configured step that goes through the command policy. **Exit criterion:** on a repository with a SCIP
index, the share of `calls` and `references` edges that are resolved, and a test showing a name collision that syntax alone gets wrong and
SCIP gets right.

### Phase 3: the tools the model sees (medium)

A small set, each bounded and READ_ONLY:

| Tool | Answers |
|---|---|
| `graph_neighbours(symbol, edge, depth)` | Callers, callees, importers, subclasses, within a budget |
| `graph_impact(symbol or file)` | What may be affected by a change, ranked, with the edge kinds and their sources |
| `graph_path(from, to)` | How one symbol reaches another |
| `graph_tests_for(symbol or file)` | Tests that reach it, by the call and import edges |
| `graph_overview(path)` | A ranked outline of a directory (the repository map, ranked by connections, as Aider does) |

Because a model may not call a tool it is given, adoption is designed in: a short guide in the system prompt names when to use each; a
`graph_impact` hint is added to the answer of `edit_file`'s neighbours (for example, "this function has 7 callers in 4 files") as a
**notice**, which the model cannot ignore as easily as a tool it has not called; and the evaluation counts calls. **Exit criterion:** on the
phase 0 tasks, a pass-rate gain and a call rate (the share of runs that used the graph) that are both reported; if the model does not use the
tools, the design changes before more is built.

### Phase 4: history and ownership (small to medium)

Add `git` edges: which files change together (co-change counts from `git log`), who last changed a symbol, and which board issue and run
touched it (the checkpoints already record this). Co-change is a way to find hidden dependencies that no parser sees, and is cheap to compute.
**Exit criterion:** a hidden-dependency task solved by co-change that the call graph alone misses.

### Phase 5: concepts (large, optional, last)

Only if phases 0 to 4 leave a measured gap in *conceptual* questions ("where is authentication handled?"). Group symbols into
modules by graph clustering (a community detection method over the call and import graph, not an LLM), and have a model write a short
summary of each cluster once, stored with its source hash and labelled as a model's reading. Compare it with retrieval over the cluster
names and summaries. Microsoft GraphRAG's community summaries are the model for this layer; its cost, a model call per community and
re-indexing on change, is the reason it is last. **Exit criterion:** a conceptual-question set on which it beats the phase 3 tools, or the
phase is dropped.

### The window

A **Graph** view in the window, after phase 3: a neighbourhood diagram of the selected symbol (nodes by kind, edges by source and
confidence), with click-through to the file. This is for the person; the model uses the tools. It is also the quickest way to see that the
graph is wrong.

### Risks

| Risk | Answer |
|---|---|
| Wrong edges from name matching in dynamic languages | Mark them unresolved; show the mark; prefer SCIP where available; measure precision on a sample |
| A stale index misleads | Key every fact by file hash; check hashes when a tool is called; say "the index is N files behind" |
| The model does not use the tools | Measure the call rate (phase 3); notices and prompt guidance; drop what is not used |
| Gains found on large models do not appear on a 9B model | The baseline and exit criteria are on the 9B model; nothing is claimed from the papers |
| Cost of indexing a very large repository | Incremental by hash; a size cap with a clear message; indexing in the background |
| Scope growth | The five phases are separable; each ends in a decision to continue or stop |

### Not planned

An LLM-extracted graph over source code; a graph database service; sending code to a hosted indexing service; running a language's
indexer or build automatically without the person's command or an explicit configured step.

## Appendix: applying the plan to a PHP code base such as Moodle

Moodle is a very large PHP application with a plug-in architecture. The following is from general knowledge of how Moodle is laid out and
has **not** been checked against a Moodle checkout in this work; verify each point before relying on it.

- **Parsing.** Bridle parses Rust, Python and C++ today. PHP needs a tree-sitter grammar added (a new pinned dependency, vendored) and a
  `Language::Php` with its definitions, `use` imports and calls.
- **Resolution.** [scip-php](https://github.com/davidrjenni/scip-php) is a SCIP indexer for PHP. It must run at the project root, needs
  `composer.json` and `composer.lock`, and requires PHP 8.2 or later. Moodle's use of class aliasing and its own autoloader are the likely
  difficulties: the existing PHPStan extension for Moodle (`micaherne/phpstan-moodle`) exists for the same reason. Whether scip-php copes with
  a Moodle checkout is unknown and is the first thing to test.
- **Framework edges.** Much of Moodle's structure is by *convention* and not by call: plug-in callbacks found by function-name pattern
  (such as `lib.php` functions named after the plug-in), event observers and hooks declared in `db/` files, capabilities declared in
  `db/access.php`, web-service functions in `db/services.php`, scheduled tasks, language strings in `lang/` files, and tables declared in
  `db/install.xml`. A graph for Moodle would add edge kinds for these (`declares_capability`, `observes_event`, `provides_service`,
  `uses_string`, `reads_table`), each produced by a small parser of the declaring file. They are exactly the connections that a text search
  or a call graph misses, so they are where a graph would help most.
- **Scale.** The phase 1 size and time limits would need testing on a checkout of that size before anything is promised.

## Sources

- [SCIP](https://github.com/sourcegraph/scip) and [the announcement](https://sourcegraph.com/blog/announcing-scip)
- [Glean: indexing code at scale](https://engineering.fb.com/2024/12/19/developer-tools/glean-open-source-code-indexing/) and [the repository](https://github.com/facebookincubator/glean)
- [Kythe](https://kythe.io/)
- [GitHub stack graphs](https://github.com/github/stack-graphs) (archived 9 September 2025)
- [codebadger paper](https://arxiv.org/abs/2603.24837) and [repository](https://github.com/lekssays/codebadger)
- [CodexGraph](https://arxiv.org/abs/2408.03910), [RepoGraph](https://arxiv.org/abs/2410.14684), [LocAgent](https://arxiv.org/abs/2503.09089), [Code Graph Model](https://arxiv.org/abs/2505.16901), [CodeCompass](https://arxiv.org/abs/2602.20048)
- [Graphiti](https://github.com/getzep/graphiti), [LightRAG](https://github.com/HKUDS/LightRAG)
- [scip-php](https://github.com/davidrjenni/scip-php)
