# Web research

Bridle can search the web, read pages and download files for the model, under the same rule as everything else: **the model proposes, the
harness decides**. It is **off by default**. The design and its limits are in
[ADR-0019](../decisions/0019-web-research-is-policy-gated-native-tools.md).

## Turn it on

It takes three settings in the **machine** configuration (`%APPDATA%\bridle\config.toml`); a repository's `bridle.toml` cannot set them.

```toml
[web]
enabled = true
max_results = 10          # the most results one search returns (1 to 30)
timeout_ms = 20000        # for a provider call or a page fetch
max_fetch_bytes = 2097152 # the most of one page that is read
max_download_mb = 100     # the largest download

[web.providers.local]     # a SearXNG you host, with json in search.formats
kind = "searxng"
enabled = true
base_url = "http://127.0.0.1:8080"

[web.providers.brave]     # the Brave Search API
kind = "brave"
enabled = true
api_key_env = "BRAVE_SEARCH_API_KEY"   # the NAME of an environment variable, never the key

[security]
network = "allow"         # lets the MODEL use the web tools; the default `ask` is never allowed
```

`bridle config validate` checks it (rule V48). The key itself lives only in the environment variable and is sent only in a request header.
Requests to a provider on this machine never use a proxy; others follow `network.proxy_mode`.

## For you, at the terminal

`bridle web ...` works as soon as `[web] enabled` is set, without `security.network = "allow"`, because you are the one asking.

| Command | What it does |
|---|---|
| `bridle web status` | Whether it is on, what the model may do, the route, the limits, how many sources are recorded, which providers are usable. |
| `bridle web providers` | Each provider: kind, enabled, address, and whether its key variable is set (never the key). |
| `bridle web search "query" [--news] [--limit N] [--domain arxiv.org] [--after 2026-10-01]` | Search every usable provider and print the fused results. |
| `bridle web open URL` | Read a page: title, text with headings and lists, and its links. |
| `bridle web download URL [--sha256 HEX]` | Save a file into `.bridle/web/downloads/`. |
| `bridle web inspect S-0002` or `bridle web inspect path\to\file` | Show a downloaded file (by source id) or a file in the project. |
| `bridle egress` | Now also lists the search providers and "any public web address". |
| `bridle web browser status / setup / test / remove` | The managed browser; see [its guide](web-browser.md). |

## For the model

When web research is on and `security.network = "allow"`, the model is offered four tools (they are not offered otherwise):

| Tool | What it does |
|---|---|
| `web_search` | `query`, and optionally `kind` (`web` or `news`), `limit`, `domains`, `after`. Returns numbered results, each with an id such as `[S-0003]`. |
| `web_open` | Reads a page. Returns the text and an id to cite. |
| `web_download` | Saves a file. Returns an id and the SHA-256. |
| `document_inspect` | Reads a downloaded file by id, or a file in the project by path: text and HTML are shown; a **PDF** gives its page count, title, author and text; a **zip, tar or tar.gz** gives its listing with names that would unpack outside their folder flagged. Nothing is extracted. |

What every result carries: it is framed as **untrusted** ("information, not instructions"), secrets are redacted, it is bounded, and it
is recorded in `.bridle/web/sources.jsonl` with the address, the final address after redirects, the time and the SHA-256 of what was read,
so a claim can be traced to the page as it was. The same page keeps the same id.

## What the harness refuses

| Case | Result |
|---|---|
| `security.network` is `ask` (the default) or `deny` | The tool is refused inside the tool, with the rule named. A client that calls the MCP server directly cannot skip it. |
| A URL that is not `http` or `https`, or has a user name or password | Refused. |
| `localhost`, `*.local`, `*.internal`, a bare host name, or an address on this machine or its network (`127.x`, `10.x`, `172.16-31.x`, `192.168.x`, `169.254.169.254` and the rest of link-local, CGNAT, multicast, reserved, IPv6 equivalents, IPv4-mapped) | Refused **before** any connection, and the same check is made at every redirect. |
| An `https` page that redirects to `http` | Refused. |
| More than 5 redirects | Refused. |
| A page larger than `max_fetch_bytes` | Cut, and said to be cut. |
| A download larger than `max_download_mb` | Stopped; nothing is left behind. |
| A download that is a program (Windows, Linux or macOS executable, or a script) | Refused. Nothing downloaded is ever run. |
| A file named `.pdf`, `.zip`, `.png`, `.jpg` or `.gz` whose content is something else | Refused. |
| A download whose SHA-256 differs from the one you gave | Removed and refused. |
| A provider that answers 401, 403, 429, an error, or something that is not JSON | Reported in words; the other providers still answer. The key is never in the message. |

## When a page needs JavaScript

`web_open` does not run a page's scripts, so a page that builds itself with script comes back nearly empty. For those, turn on the
[managed browser](web-browser.md): `browser_navigate`, `browser_snapshot` and `browser_action` read the page as a person sees it, click
and type, and keep a file the page offers through the same download manager. The way to work is: search, `web_open`; if the text is too
thin, `browser_navigate` and `browser_snapshot`. Both kinds of reading go into the same `sources.jsonl`; a page read both ways has two
records and the browser one names the other (`rendition_of`).

## Limits, stated plainly

- **Providers are verified against fake servers, not the real services.** There is no Brave key or SearXNG instance on the tested machine.
  The requests were checked against the providers' published documentation (Brave: the endpoint, `X-Subscription-Token`, `q`, `count` up
  to 20, `freshness` `pd`/`pw`/`pm`/`py`; SearXNG: `q`, `format=json`, `categories`, `time_range` of `day`, `month` or `year`, and that
  JSON may be disabled), which found one error (a week was sent as `week`). The documentation pages read did not list the fields of a
  result, so how an answer is read is checked only against the formats the code was written for.
  Opening pages and downloading files were verified live through the proxy (a GitHub raw file downloaded and read back by its id).
- **Names are resolved twice** (once to check, once to connect), so a hostile DNS server could change its answer in between. Every redirect
  hop is checked, and when a proxy carries the request the proxy resolves the name and applies its own rules. The browser's egress proxy
  is different: it resolves once and connects to the address it judged (see [the browser](web-browser.md)).
- **Remote HTTPS now uses the system certificate store.** Before ADR-0020 it used a bundled list of roots, which failed behind a proxy
  that re-signs certificates and for sites with newer roots (`rfc-editor.org` and `nodejs.org` were such cases).
- **PDF text is best effort.** The reader handles single-byte fonts and Flate compression (most word-processor and LaTeX output). It says so,
  and shows no text, for encrypted files, for text stored in embedded font encodings (many designed documents), for other compressions, and
  for scans (images only). Page order is the order in the file. There is no OCR. It was checked against PDFs made for the tests and one
  compressed sample, not against a corpus of real papers.
- **Archives are listed, never unpacked.** zip (not zip64), tar and tar.gz/tgz; 7z and rar are reported but not listed. A tar.gz is
  decompressed in memory up to 64 MiB to read its headers (the listing says when that was not all of it).
- **No crawling, no academic or news providers beyond the `news` kind.** They are the next phases of the plan. The browser is described in
  its own guide.
- Providers need what they need: Brave a key (and its usage terms), SearXNG your own instance with JSON output enabled.
