# The managed browser

Bridle can open a web page in a real Chromium, let the page run its JavaScript, read what a person would see, click and type, and keep a
file that a page offers for download. It is an addition to [web research](web-research.md), not a replacement: reading a page with
`web_open` is faster and costs nothing, and the browser is for pages that return too little that way. It is **off by default**, **optional
to install**, and **a capability that Bridle controls**: the model gets four tools with a short list of verbs, and every connection the
browser makes goes through a proxy that Bridle runs and Bridle's rules judge. The design, the alternatives and what is not closed are in
[ADR-0020](../decisions/0020-managed-browser-is-a-policy-gated-playwright-worker.md).

## How it fits together

```text
Model / OpenCode / any MCP client
        |  browser_navigate, browser_snapshot, browser_action, browser_close
        v
Bridle MCP tools  --- policy gate inside every tool (security.network), config gate, argument checks, event log
        |
        v
Browser manager (Rust)  --- sessions, deadlines, folders, limits, identity check of the installed programs
        |   JSON lines on a pipe: launch, navigate, snapshot, action, screenshot, close
        v
Worker (Node.js + Playwright, one per session)  --- fixed launch flags, sandbox on, ephemeral profile, no scripts from outside
        |
        v
Chromium  ---> ONLY route: Bridle's egress proxy on 127.0.0.1  ---> judged per connection ---> the web
        |
        v
Results come back to Rust: text extracted by the existing extractor, files through the existing download manager,
everything recorded in .bridle/web/sources.jsonl
```

## Turn it on

Three steps, all yours. Nothing is installed by using Bridle.

1. In the **machine** configuration (`%APPDATA%\bridle\config.toml`; a repository's `bridle.toml` cannot set any of this):

   ```toml
   [web]
   enabled = true               # the browser is part of web research (it also needs a provider; see web-research.md)

   [web.browser]
   enabled = true
   max_sessions = 2             # each session is its own Chromium (1 to 4)
   navigation_timeout_ms = 30000
   action_timeout_ms = 10000
   session_idle_secs = 600      # an unused session is closed
   max_page_content_chars = 20000
   screenshots_enabled = false  # off until you want pictures
   downloads_enabled = false    # off until you want files the page offers
   allowed_hosts = []           # e.g. ["arxiv.org", "doi.org"]: if not empty, ONLY these names and what is under them
   blocked_hosts = []           # names the browser may never reach; wins over allowed_hosts
   allow_unpinned_destinations = false   # see "Behind your own proxy" below

   [security]
   network = "allow"            # lets the MODEL use the browser; the default `ask` is never allowed
   ```

2. Install the runtime (this downloads about 35 MB of Node.js, 4 MB of playwright-core and about 170 MB of Chromium; 560 MB once unpacked, through your proxy):

   ```text
   bridle web browser setup
   ```

3. Check it, without the internet:

   ```text
   bridle web browser test
   ```

`bridle config validate` checks the settings (rule V49). `bridle doctor` shows a `browser` line, and `bridle egress` lists the browser
as a destination.

## Commands

| Command | What it does |
|---|---|
| `bridle web browser status` | Enabled or not, the runtime state (`not installed`, `incomplete`, a version mismatch, or installed), the versions and the SHA-256 of `chrome.exe`, the limits, and how the browser reaches the web. Exit code 1 if the runtime is not ready. |
| `bridle web browser setup` | Downloads and installs the pinned runtime into `browser-runtime\` next to the machine configuration. Safe to run again: a matching installation is checked and left alone; anything else is cleaned and installed again. |
| `bridle web browser test` | Starts a real Chromium against a small site served from this machine and runs about 70 checks: pages, script-built pages, forms, keys, downloads, screenshots, session isolation, and a hostile page that tries to reach the inner network, a file and Bridle's own output. Needs no internet. |
| `bridle web browser test --live https://example.com` | The same, plus one page on the real network through your route. An optional smoke check, not part of the deterministic test. |
| `bridle web browser remove` | Deletes the runtime folder. It refuses a folder that is not named `browser-runtime`. |

## What the runtime is

| Piece | Version | Where from | Checked against |
|---|---|---|---|
| Node.js | 24.22.2 (Windows x64) | nodejs.org/dist | the SHA-256 that nodejs.org publishes |
| playwright-core | 1.64.1 (Apache-2.0, no dependencies) | registry.npmjs.org | the SHA-512 `integrity` that npm publishes |
| Chromium | 156.0.8078.4 (Playwright revision 1248) | Playwright's own installer | Playwright's download over TLS; Bridle records the SHA-256 of `chrome.exe` and checks it, and `node.exe`, before every start |

The three pieces are downloaded on request and are **not distributed with Bridle**: Node.js is MIT-licensed, `playwright-core` is
Apache-2.0 (and has no dependencies of its own), and Chromium carries the Chromium Project's BSD-style licences with those of its bundled
components. Bridle itself added no Rust dependency for the browser.

The versions are fixed by the Bridle you run; a different version on disk is reported as a mismatch, not used. The manifest
(`browser-runtime\manifest.json`) records versions, hashes, the time and the platform. The worker program is part of the Bridle program
itself and is rewritten from it before every start.

## The tools

All four are offered only when `[web] enabled` and `[web.browser] enabled` are on. Page content is always returned as **untrusted
text**: framed between markers with a random id the page cannot know, redacted, bounded, and recorded.

| Tool | Policy | What it does |
|---|---|---|
| `browser_navigate` | NETWORK | `url`, optional `session_id` (omit to start a session), optional `wait_until` (`commit`, `domcontentloaded` (default) or `load`). Returns the session id, the final address, the HTTP status, the title, the redirect count, the hosts contacted, and anything the proxy refused. |
| `browser_snapshot` | NETWORK | `session_id`, and optionally `include_links`, `include_metadata`, `include_accessibility`, `max_content_chars`, `screenshot`. Returns the source id `[S-0004]`, the page text after its scripts ran, links, metadata (description, canonical, language, author, published) and an accessibility outline (roles and names, which is how `browser_action` finds things). |
| `browser_action` | NETWORK | `session_id`, `action` and what it needs. `click`, `fill`, `select`, `scroll`, `press`, `wait`, `wait_download`, `back`, `forward`. |
| `browser_close` | read-only | `session_id`. Closes Chromium and removes the session's folder. Always allowed, so a session can be freed even when the network is denied. |

`browser_action` finds an element **by what a person sees**, never by a selector:

```json
{"session_id": "br-…", "action": "click", "target": {"role": "button", "name": "Continue"}}
{"session_id": "br-…", "action": "fill",  "target": {"label": "Your name"}, "value": "Ada"}
{"session_id": "br-…", "action": "press", "key": "Enter", "target": {"placeholder": "Search"}}
{"session_id": "br-…", "action": "click", "target": {"text": "Next page", "index": 1}}
```

- A target is exactly one of `role` (with an optional `name`), `label`, `placeholder` or `text`; `exact` and `index` refine it.
- If **several** elements match, the answer says how many and shows the first few. Nothing is guessed: add `index`.
- If none matches, the answer says so after waiting up to `action_timeout_ms`.
- There is **no** script evaluation, no raw DevTools command, no file upload, no change to how Chromium starts, and no typing into a
  password field. Keys are `Enter`, `Tab`, `Escape`, `Backspace`, `Delete`, `Space`, the arrows, `PageUp`, `PageDown`, `Home`, `End`,
  `Shift+Tab`, `Control+A`, or one letter or digit.
- `wait_download` waits for a download the page is about to start (after a click that triggers one the tool also waits a moment).

**Downloads.** A file a page offers is kept only when `downloads_enabled`. It is saved by the browser into the session's own folder
under a name Bridle's worker chose, then taken by the **same code as `web_download`**: the size limit (`web.max_download_mb`), what the
first bytes say it is (a program is refused, a name that lies about the type is refused), the SHA-256, an atomic move into
`.bridle/web/downloads/`, and a source record. A file already there is "already there". The page cannot choose where anything is written.

**Screenshots.** Only when `screenshots_enabled`, and only of the visible window (1280 by 800). A PNG up to 4 MiB is checked for its
signature, hashed and kept in `.bridle/web/screenshots/`, with a source record. Nothing but the window's pixels is captured.

## What is recorded

Every call is one line in `.bridle/tool-events.jsonl`: the tool, the policy verdict, the duration, the status, a label of the session (not
its id, which is a capability), the requested and final addresses, the hosts contacted, the count of refused requests, redirects, and the
source ids. A refusal by the policy is logged with the rule. Page content, cookies and headers are never logged.

Each page read is a record in `.bridle/web/sources.jsonl` with `extraction: "browser"`, the address asked for and the final one, the time,
the page title, the content type, the Chromium and Playwright versions, the session, and the SHA-256 of the rendered HTML. If the same
address was read earlier without the browser, the new record names it (`rendition_of`).

## The boundaries

| Question | Answer |
|---|---|
| Who may call a tool? | Only a call the policy allows (`security.network = "allow"`), checked **inside** each tool, so a client that talks to the MCP server directly cannot skip it. The default `ask` is never allowed. Unknown tool names stay blocked. |
| Can the model give Chromium a flag, a script or a path? | No. The protocol has no such field, and the worker holds the launch flags as constants. |
| Where can the browser connect? | Only through Bridle's egress proxy, which judges each connection, per session, and only where `allowed_hosts` / `blocked_hosts` let it. See below. |
| Whose profile does it use? | None of yours. Each session has a throw-away profile under its own folder, no persistent cookies, no service workers, no permissions, no extensions, and Chromium's sandbox stays on. Two sessions share nothing (tested: a cookie and local storage in one are invisible in the other). |
| Can one task use another's session? | A session id is 64 random bits and belongs to the server instance and the workspace that made it. A wrong id, a closed one and an expired one give the same answer. |
| What if it crashes or hangs? | Every request has a deadline. A silent, crashed, malformed or exited worker is killed and the session says why on the next call; the model can open a new one. When Bridle ends, every session is closed and no Chromium is left (checked). |
| Cookies and credentials | Bridle gives the model no access to cookies or headers; it sees only text a page chooses to print, and output redaction masks values under names such as `cookie`. |

### The egress proxy

Chromium is started with `--proxy-server` pointing at Bridle's proxy on `127.0.0.1`, name lookup switched off outside it, QUIC off and
WebRTC limited to the proxy. The proxy signs each session in with a secret, then for every connection:

- refuses Chromium's own service endpoints (updates, accounts, safe browsing, translation, autofill) quietly: they are counted in the log
  (`browser_internal_refused`), not reported to the model;
- applies your `blocked_hosts` and `allowed_hosts` before any lookup (a name matches itself and what is under it; with `allowed_hosts`
  set, a bare address is refused too);
- refuses numbers and names that mean this machine or its network, in any spelling: `127.0.0.1`, `localhost`, `localhost.`, `[::1]`,
  `[::ffff:127.0.0.1]`, `fe80::`, `169.254.169.254`, `10.x`, `192.168.x`, `2130706433`, `0x7f.1`, `0177.0.0.1`, `127.1`, `*.local`,
  `*.internal`; and ports of mail, shells and databases;
- **resolves a name once, judges every answer, and connects to an address it judged**, so a name cannot be re-pointed between the check
  and the connection (DNS rebinding);
- applies to everything the browser does: the page, scripts, images, frames, redirects, WebSockets and `fetch`;
- cuts a call that moves more bytes than its limit.

**Behind your own proxy** (the only way out on some networks) Bridle still resolves the name first, judges every answer, and asks your
proxy for a tunnel **to the address it checked** (`CONNECT 93.184.216.34:443`), not to the name; TLS inside the tunnel still verifies the
real name. A name that turns hostile between two requests is refused, and a proxy that refuses the tunnel to the address gets no second
try by name. If this machine **cannot resolve** the name (some company networks resolve only at the proxy), the connection is refused
and the answer says why. Setting `allow_unpinned_destinations = true` hands such names to your proxy instead: it is counted, told to
the model ("N connection(s) could not be tied to an address Bridle checked"), and for those connections your proxy is the boundary.
`bridle web browser status` says which mode is in force. Requests the proxy refuses are counted and described in the next tool answer.

## Limits, stated plainly

- **Real Chromium results are from one machine** (Windows 11, x64). The self-test passed 73 of 73 checks and the live check read
  `https://example.com` through the machine's company proxy. It has not been run on another machine, another proxy or another Windows.
- **DNS rebinding is shown against a scripted resolver, not a hostile DNS server.** Aiming the system resolver at one means changing
  the machine's DNS settings, which was not done. In both modes (direct and behind a proxy) the connection goes to an address Bridle
  judged, once per request; the exception is `allow_unpinned_destinations`, which you must turn on yourself.
- **Chromium's own connections.** A net log showed Chromium reaching `www.google.com` (a search-engine preconnect and an "AI mode"
  check), `clients2.google.com` (a time query), `update.googleapis.com` and `accounts.google.com` by itself. The first three are
  switched off by flags (Playwright's list plus three more; a self-test check fails if Chromium contacts anything of its own in the
  first seconds), and the service endpoints are refused by the proxy whatever the flags do. A future Chromium may add new ones;
  `allowed_hosts` is the strict answer.
- **No persistent login, no uploads, no frames in the snapshot (their number is shown), no PDF viewer.**
- **Full-page screenshots, video and tracing are not offered.**
- **A hard kill of Bridle** is cleaned up by the pipe closing, not by a job object: a test aborts a Bridle process that holds a session
  and Chromium is gone within seconds. The dead process's session folder is removed by the next Bridle that starts.
- **The install is Windows x64 only.** Chromium's download is not hash-pinned by its publisher; the pin is Bridle's own, of the whole
  installed folder (taken from a clean install and reproduced by two later ones), and `setup` refuses a Chromium that differs. It is not an
  independent attestation.
- The runtime is about 560 MB on disk (measured: Node.js, playwright-core and the full Chromium; the separate headless shell is not installed).
