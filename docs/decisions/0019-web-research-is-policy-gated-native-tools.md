# ADR-0019: Web research is a set of native, policy-gated tools with an evidence record

- **Status:** Accepted (2026-11-04); the first milestone (v0.1) is built
- **Date:** 2026-11-04
- **Deciders:** Kushagra Srivastava

## Context

Bridle needs web research: search, reading pages, downloading papers and files, and answers that can be traced to their sources. The
owner supplied a detailed plan (a Rust-native coordinator, pluggable providers, optional specialist runtimes, Bridle's policy on every
call, an evidence store, six phases, a first milestone). That plan asks for a design grounded in this repository first, and for the
design to challenge the plan where it should. This record is that design.

## What the repository already has

| Component | State |
|---|---|
| `ToolKind::WebFetch` / `WebSearch` | Classes OpenCode's own `webfetch` and `websearch` as NETWORK; the verdict is `security.network`, default `ask`, which the harness never allows. |
| `ToolKind::Harness(name)` | Every Bridle MCP tool. A name that is in no table is `unknown_tool`: refused. |
| `harness_runtime::http` | `ureq` with native TLS, the proxy route (`network.proxy_mode`, `BRIDLE_PROXY`), loopback that never uses a proxy. |
| `mcp::tools` | `McpTool` trait, `redacted_output`, atomic writes; `run_command` checks the policy **inside the tool**. |
| `bridle egress`, `config trust` | An audit of destinations; machine-only settings that a repository cannot set. |
| Plugins (ADR-0013) | Declared fixed commands; network-class tools are excluded by design. |

## Answers to the six questions of the plan

1. **How do new MCP tools pass the host hooks and policy?** In the OpenCode host and in the chat, an MCP tool is `ToolKind::Harness(name)`.
   The policy now lists `web_search`, `web_open` and `web_download` as NETWORK (`WEB_NETWORK_TOOLS`, rule `web_tool`) and
   `document_inspect` as READ_ONLY. A look-alike name is `unknown_tool` and refused (tested).
2. **How is a direct MCP client prevented from bypassing that?** The hook is not enough: `Server::call` does not consult it. Each web tool
   calls `ctx.policy.evaluate(...)` as its first step and refuses unless the verdict is `allow`; and the tools are not even offered unless
   `[web] enabled`. Tested: with `ask` or `deny` every call is refused with the rule named, and with `allow` an address on this network is
   still refused.
3. **What can be reused?** `http::agent_for` (extended with a no-redirect variant), the proxy route, `redacted_output`, `sha2`, the
   `.bridle/` state folder, `egress`, the machine-only trust rule (`web` is refused in a repository's file, like `network`), validation
   (rule V48). Not reusable: the sandbox (ADR-0012 is not built on Windows), and nothing executes in v0.1.
4. **How are optional runtimes installed, pinned and verified?** v0.1 has none. Crawl4AI, Playwright, Docling and GROBID are later, as
   optional **out-of-process services on a loopback endpoint** named in the machine configuration (like SearXNG), each with a pinned
   version and checksum recorded the way `bridle models manifest` records a model. Bridle never bundles or starts them silently.
5. **Which parts need new classifications?** The four tool names above, and a new rule, `web_tool`. Nothing else: see the challenge on
   two-dimensional permissions below.
6. **What is the minimum viable implementation without unnecessary dependencies?** No new crates: `ureq` (HTTP, TLS, proxy), `sha2`,
   `serde_json`, `std::net` for address checks, a hand-written HTML text extractor, and the `Fetcher` and download manager written here.
   PDF text extraction needs a crate (or a service) and is **not** in v0.1.

## Where this design differs from the plan, and why

- **Eight tools became four, and news is a mode.** Every tool's schema is paid for in the model's context on every request, and the models
  here have small windows. v0.1 offers `web_search` (with `kind: web|news`, `domains`, `after`), `web_open`, `web_download` and
  `document_inspect`. Academic search (`paper_search`) is a later provider kind behind the same tool or a fifth tool, not a new subsystem.
- **The address guard lives in `harness-runtime`, not `harness-core`.** It needs `ureq`'s URI type and DNS; `harness-core` stays free of
  network code. Configuration, validation and trust live in core, as planned.
- **A download is one permission, not two.** The plan asks for a two-dimensional check (network and write). `web_download` writes only into
  the harness-owned `.bridle/web/downloads/`, which the model cannot choose, so the second dimension is constant. A tool that writes to a
  path the model chooses (archive extraction) will need the write check; it is not built.
- **Checking a name and connecting to it are two steps.** There is no hook in `ureq` to pin the connection to the checked address, so a
  hostile DNS server could answer differently the second time (rebinding). The mitigation: every hop of every redirect is checked; literal
  and local-looking addresses are refused without DNS; when a proxy carries the request, the proxy resolves the name and applies its own
  rules (so names are not resolved locally, because on a proxied network local DNS may not work at all). This is recorded as a residual risk.
- **Providers need credentials or an instance that this machine does not have.** Brave needs an API key and SearXNG a running instance, so
  the provider code is verified against local fake servers, not against the real services (said plainly in the guide). The page fetch and
  the download were verified live through the proxy.
- **`robots.txt` and pacing are for crawlers.** v0.1 fetches single pages the model names; the robots check, domain pacing and crawl
  budgets arrive with `web_crawl`.
- **Reciprocal rank fusion, per-site caps and canonical addresses are built;** an LLM-based re-ranker is not.

## Decision

1. Web research is built as native tools in `harness-runtime` (`web/` and `mcp/tools/web.rs`) behind `[web] enabled`, off by default, in the
   machine configuration only.
2. Every call is checked by the policy inside the tool; the default (`security.network = ask`) blocks it. The person's own `bridle web`
   commands skip that gate (the person is the actor) but not the address guard.
3. Every URL and every redirect hop is checked before it is contacted. Only `http` and `https`, no credentials, and no address on this machine
   or its network (loopback, private, link-local and cloud metadata, CGNAT, multicast, reserved, IPv6 equivalents, IPv4-mapped).
4. Everything a page or a provider returns is framed as untrusted, redacted, bounded, and **recorded** in `.bridle/web/sources.jsonl` with its
   address, final address, time and SHA-256, and cited by an id (`S-0007`).
5. A download is streamed, size-limited, checked against an expected SHA-256 when given, identified by its first bytes, refused if it is a
   program or if its name lies about its type, and moved into place only when complete. Nothing downloaded is run.

## Consequences

- Search works only with a provider the person configures. Brave and SearXNG are built; Tavily, Exa, OpenAlex, Semantic Scholar, arXiv,
  Crossref and GDELT are further adapters of the same trait.
- The first milestone does not read PDFs, unpack archives, crawl, drive a browser, or install anything. Those are phases 2 to 5 of the plan.
- A fetch is only as safe as the address check plus the proxy: see the rebinding note.

## Not built yet (from the plan)

`paper_search` and news providers beyond the `kind` switch, PDF extraction (Docling, GROBID), archive inspection and extraction with
zip-slip and bomb checks, `web_crawl` (Crawl4AI), a browser runtime (Playwright), a research-session artifact store beyond the source
record, citation checking, and the evaluation suite of about 100 tasks.
