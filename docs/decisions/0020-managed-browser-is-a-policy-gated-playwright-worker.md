# ADR-0020: The browser is a managed Chromium behind Bridle's own egress proxy, driven by a worker that Bridle starts

- **Status:** Accepted (2026-11-04); the first milestone is built and was run against a real Chromium
- **Date:** 2026-11-04
- **Deciders:** Kushagra Srivastava

## Context

Web research v0.1 ([ADR-0019](0019-web-research-is-policy-gated-native-tools.md)) reads pages with Bridle's own HTTP client. That client
does not run a page's JavaScript, so a page that builds itself with script comes back nearly empty, a "Download" button that is a script
does nothing, and nothing can be clicked. The owner asked for Chromium, managed through Microsoft Playwright, inside that subsystem, with
one rule above the others: **the browser must be a capability controlled by Bridle, never an authority independent of Bridle.**

## What the repository already has (inspected, not assumed)

The reported state was checked first: branch `web-research`, commits `d28b0a2` and `e49df81`, a clean tree except for old untracked
benchmark folders that were left alone.

| Component | State | Used for the browser |
|---|---|---|
| `ToolKind::Harness(name)`, `WEB_NETWORK_TOOLS`, rule `web_tool` | `web_*` are NETWORK; an unknown name is `unknown_tool`, never allowed | The four browser tools are added to the same table; look-alikes stay blocked (tested) |
| Gate inside the tool (`mcp::tools::web::gate`) | Each web tool calls the policy itself | Reused by every browser tool, because `Server::call` does not run the host hook |
| `web::guard` | Judges a URL and the addresses of a name | Extended (see below) and called by the egress proxy for every connection |
| `web::download` | Streams, caps, sniffs, hashes, moves into place | Split so a browser download goes through exactly the same code (`ingest_file`) |
| `web::store` (`sources.jsonl`) | Append-only source records with SHA-256 | Extended with `extraction`, browser and Playwright versions, session and `rendition_of`; no second evidence store |
| `web::html::extract` | HTML to readable text and links | Reused on the rendered DOM; the worker does not extract text itself |
| `http::RemoteRoute`, `agent_for_redirects` | The proxy route, `ureq` with native TLS | Reused for the setup downloads; its proxy decides the browser's upstream |
| `config::trust` | `web` and `network` are refused in a repository file | `[web.browser]` is inside `[web]`, so it is machine-only with no new code |
| `event_log` (`tool-events.jsonl`) | Redacted append-only events | Every browser call and every refusal is written there |
| `process::hide_console`, `launcher` | Child processes on Windows | Pattern reused; no new unsafe code |
| `bridle egress`, `bridle doctor` | Destination listing, environment report | Both report the browser |
| Runtime installation | **None.** `llama-server` and `opencode` are found by path; `bridle models manifest` records model files | A new, small managed-runtime installer (below) |
| Node.js | **Not on this machine** and not a Bridle dependency | Pinned and installed only into the managed folder, only on request |

`unsafe_code = deny` holds, and **no new Rust dependency was added** (the build is `--locked --offline` from a vendored set).

## Decisions

### 1. Playwright, driven by a Node worker per session

The preferred design was challenged and kept. The alternatives:

| Option | Why not |
|---|---|
| A Rust CDP client (`chromiumoxide`, `headless_chrome`) driving a downloaded Chromium | Would drop Node, but every action (locators that wait, strictness, downloads, popups, auto-waiting) would be re-implemented; a new dependency tree; the owner asked for Playwright |
| Python Playwright | Not a Bridle dependency either; it bundles Node anyway |
| The installed Microsoft Edge (`channel: "msedge"`) | No 170 MB download, but not pinned, updates itself, and a user profile is one flag away. Not a reproducible, verifiable runtime |
| An off-the-shelf Playwright MCP server beside Bridle | Exactly what the rule forbids: a second authority with its own tools (evaluate, CDP, file upload) the policy cannot see |

**One worker process and one Chromium per session** (not one browser with many contexts). Cost: about 150 to 250 MB of memory for each of
at most `max_sessions` (default 2). Gain: sessions share no process, a crash ends one session and not the others, there is nothing to
leak between them, and every connection can be tied to a session by its proxy credentials.

### 2. A closed line protocol, with no paths and no code

Bridle and the worker speak JSON lines (version 1, 8 MiB per line). The operations are `launch`, `navigate`, `snapshot`, `action`,
`screenshot` and `close`. No operation takes a file path, a script, a DevTools command, a program or a launch flag:

- The worker's launch arguments, the sandbox setting (`chromiumSandbox: true`: Playwright turns it off by default), the extensions,
  quic, WebRTC and name-lookup flags are constants in the worker.
- Elements are found by a role and name, a label, a placeholder or some text (an allow-list of 45 ARIA roles). There is no selector,
  no `evaluate`. Keys are an allow-list. `fill` into a password field is refused.
- Files the browser produces go into folders Bridle created for the session, under names the worker chose (`dl-N.bin`, `shot-N.png`).
  Bridle checks the pattern of every name the worker reports and reads only from the folder it made. A page's suggested file name is
  only a label.
- The worker is built into Bridle (`include_str!`) and rewritten from that copy before every start, so a file on disk cannot replace it.

A worker that is silent past its deadline, writes a malformed line, answers the wrong request, exits, or reports a crash is killed and
its session is over. Nothing falls back to another mode.

### 3. The network boundary is Bridle's own proxy, not a check on the first URL

Validating the first URL is not enough: a page makes hundreds of requests, follows redirects, opens frames and WebSockets. So Chromium
is started with **one** route, a proxy that Bridle runs on `127.0.0.1` (random port), with `--host-resolver-rules=MAP * ~NOTFOUND` so no
name can be looked up outside it, QUIC off and WebRTC limited to proxied UDP. Playwright's per-launch `proxy` carries a user name (the
session id) and a secret that exists only in that session's launch message, so the proxy knows which session a connection belongs to,
no other program on the machine can use it, and a closed session's access ends at once.

For every `CONNECT` (HTTPS, WebSockets) and every plain `http://` request the proxy decides, in this order, with no name looked up
until the earlier steps have passed:

1. **Ports** of mail, shells, databases and the like are refused.
2. **Chromium's own service endpoints** (updates, accounts, safe browsing, translation, autofill, `clients*.google.com`) are refused
   without a word to the model; they are counted and logged (`browser_internal_refused`). A net log showed Chromium reaching
   `update.googleapis.com` and `accounts.google.com` by itself, which a page never needs.
3. **The person's lists**: `web.browser.blocked_hosts` always wins; if `allowed_hosts` is not empty, only those names (and what is
   under them) are reachable, and a bare address is not. A name matches itself and its subdomains, not a name that only ends in the
   same letters.
4. **The address**: numbers are judged as numbers, **including the old spellings** (`2130706433`, `0x7f.1`, `0177.0.0.1`, `127.1`),
   IPv6 (`::1`, `::ffff:127.0.0.1`, `fe80::`, `fc00::`, zone ids), `localhost.` with a trailing dot, `.local`, `.internal` and the
   rest. A name is resolved **once**, every answer is judged (one bad answer refuses the name), and the connection is made to an
   address that was judged, never to the name again. A script that answers "public" first and "127.0.0.1" afterwards has no second
   lookup to win (tested with a scripted resolver).

Each call also has a byte limit (the download limit plus 64 MiB), which bounds what a download can write to disk, and what is refused is
counted and named, and told to the model on its next call.

**Behind the person's own proxy** (the only way out on the tested machine) the same steps run, and the tunnel is then asked for **at the
checked address** (`CONNECT 93.184.216.34:443`), not at the name: TLS inside the tunnel still verifies the real name, so it works, and
the upstream proxy cannot be talked into a different address by a name that changes its answer. This was run for real: Chromium read
`https://example.com` through the campus proxy this way. If the upstream refuses the tunnel to the address, the connection fails and
the name is **not** tried instead. If this machine cannot resolve a name at all, the connection is refused, unless the person sets
`web.browser.allow_unpinned_destinations = true`; then the name is handed to the proxy, the connection is counted and told to the
model, and for those connections the proxy is the boundary. `bridle web browser status`, `bridle egress` and the self-test say which.

**What this still does not show.** Rebinding is demonstrated against a scripted resolver, in both modes, with real sockets. It has not
been run against a hostile DNS server answering the system resolver, because pointing the system resolver at one means changing the
machine's DNS settings, which this work did not do.

### 4. The managed runtime is optional, pinned and re-checked

`bridle web browser setup` installs three pinned pieces (a clean reinstall gave the same `chrome.exe` SHA-256 both times) into `browser-runtime\` next to the machine configuration, and nothing else:

| Piece | Pin | Verified against |
|---|---|---|
| Node.js 24.22.2 (win-x64 zip, MIT) | URL and SHA-256 in the source | The SHA-256 nodejs.org publishes (`SHASUMS256.txt`) |
| `playwright-core` 1.64.1 (npm tarball, Apache-2.0, no dependencies) | URL and SHA-512 in the source | The `integrity` npm publishes |
| Chromium 156.0.8078.4 (Playwright revision 1248, BSD-style licences) | Version and revision; installed by Playwright's own installer; **the SHA-256 of the whole installed folder (304 files: path, size, hash of each) is pinned in Bridle's source** | The folder hash at install time, which `setup` refuses to accept if it differs. The pin is Bridle's own, taken from a clean install and reproduced by two later clean installs on this machine; neither Playwright nor the Chromium project publishes a hash. Before every start Bridle re-checks `node.exe`, `chrome.exe` and `chrome.dll` (where nearly all of Chromium's code is) against the install-time hashes |

Unpacking uses the `tar.exe` that ships with Windows, by full path. The manifest is written last; a half-finished install is visible
(`incomplete`) and the next `setup` or `remove` cleans it. A repository cannot choose any path or version. Nothing is installed by first
use; a missing runtime is reported with the command that fixes it. The proxy of `network.proxy_mode` is used for the downloads.

### 5. Provenance is the existing registry

A snapshot is a `page` source with `extraction: "browser"`, the Chromium and Playwright versions, the session, the address asked for and
the final address, and the SHA-256 of the rendered HTML. Reading an address the plain fetcher already read adds a second record that
names the first (`rendition_of`): they are two readings of one address and both are kept. Reading the same rendering again returns the
same record. A browser download is a `download` source made by the same code as `web_download`; a screenshot is a `screenshot` source.

### 6. Authorization

`browser_navigate`, `browser_snapshot` and `browser_action` are NETWORK: `security.network = allow` is required, `ask` is never allowed.
`browser_close` is read-only so a session can always be freed. Writing is not implied by network approval: a download is kept only if
`downloads_enabled`, only into the fixed `.bridle/web/downloads/` (which the file tools cannot write: `.bridle` is protected), under the
same type, size and checksum rules, and the page cannot name where.

## Findings while building it (fixed here)

- **`ureq` was not using the system certificate store**, whatever the comment in `http.rs` said: with `native-tls` its default is its
  own bundled list of roots, with the system's switched off. Behind a proxy that re-signs certificates with a company root, and for any
  site whose root is newer than that list, every HTTPS request failed ("unable to find any user-specified roots"). This is why
  `rfc-editor.org` and `nodejs.org` failed in earlier checks. Now `RootCerts::PlatformVerifier`.
- **`localhost.` (trailing dot) passed the web guard when a proxy carried the request**, and so did old numeric spellings of IPv4
  addresses. Both are now refused in `guard`, for the plain web tools too.
- Bridle's own output redaction masks a value under a key named `session` or `cookie`. A structured field named `session_id` was
  rewritten to `[REDACTED]`, so the first version of the tools returned an id nobody could use. The id is now under `browser`, and the
  log carries a label derived from it, not the id (an id is a capability).
- Chromium reached out to `www.google.com:443` and, later, to `update.googleapis.com` and `accounts.google.com` on its own. Cause:
  the worker passed its own `--disable-features=...`, and Chromium keeps only the **last** such flag, so it replaced Playwright's
  list and switched back on `NetworkTimeServiceQuerying` and `AimEnabled` (found from Playwright's switches and a net log). The worker
  now sends the union of Playwright's list for the pinned version and its own, plus the three search-preconnect features found in
  `chrome.dll`. The self-test fails if Chromium contacts anything on its own in the first seconds, so a change of pin that brings
  this back is caught; and the proxy refuses the service endpoints regardless.
- SearXNG documents `time_range` as `day`, `month` or `year`; the provider sent `week`. A week is now asked for as a month.
- A Bridle that is ended without clean-up leaves its session folders. They are now named `<pid>-<id>`, and a folder whose process no
  longer runs is removed when the next manager starts.

## Consequences

- A page can now be read as a person sees it, acted on, and cited with a checksum, without any new authority for the model.
- A new moving part: Node plus Chromium, 560 MB on disk (measured after a clean install), optional, removable with `bridle web browser remove`.
- Sessions cost memory. Two at once is the default and the limit is 4.

## Not done, and why

- **No persistent profile, no logged-in state, no uploads, no iframes in the snapshot, no PDF viewer, no extension.** The tools say so.
- **No Windows job object** to kill a worker's whole process tree if Bridle is killed hard: that needs `unsafe`. Instead the worker watches
  its pipe and Chromium is started over a pipe. This was **tested**: a child that opened a session was aborted with no clean-up, and
  Chromium and Node were gone within seconds (`tests/browser_live.rs`). Not tested: `TerminateProcess` of a parent while the worker is
  in the middle of a long page load, or a kill of the machine.
- **Not run on Linux or macOS**; the pins are for Windows x64 and `setup` says so elsewhere.
- Chromium's own download of its binary is not hash-pinned by the publisher (see decision 4).
