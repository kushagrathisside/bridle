'use strict';
// Bridle browser worker (ADR-0020). One worker drives ONE Chromium process for ONE browser session.
//
// It is started, supervised and stopped by Bridle's Rust runtime (web::browser) and speaks to it over stdin/stdout in a small,
// versioned protocol: one JSON object per line. It offers a fixed list of operations (launch, navigate, snapshot, action,
// screenshot, close). There is no operation that evaluates caller-supplied code, sends a raw DevTools command, runs a program,
// or opens a path: the protocol carries no file path at all. Files the browser produces go into folders under BRIDLE_BROWSER_DIR,
// which the Rust side created, under names this worker chose (`dl-N.bin`, `shot-N.png`).
//
// Everything the browser may reach is decided OUTSIDE this file: Chromium is forced through Bridle's egress proxy, which judges
// every connection. The checks here (schemes, key names, roles) are a second layer, not the first.

const path = require('node:path');
const fs = require('node:fs');
const readline = require('node:readline');

const PROTOCOL = 1;
const MAX_OUT = 6 * 1024 * 1024; // the longest reply line, below the Rust side's limit
const ROLES = new Set([
  'alert', 'article', 'banner', 'button', 'cell', 'checkbox', 'columnheader', 'combobox', 'complementary', 'contentinfo', 'dialog',
  'figure', 'form', 'grid', 'gridcell', 'group', 'heading', 'img', 'link', 'list', 'listbox', 'listitem', 'main', 'menu',
  'menuitem', 'navigation', 'option', 'radio', 'region', 'row', 'rowheader', 'search', 'searchbox', 'slider', 'spinbutton',
  'status', 'switch', 'tab', 'table', 'tablist', 'tabpanel', 'textbox', 'tree', 'treeitem',
]);
const KEYS = new Set([
  'Enter', 'Tab', 'Escape', 'Backspace', 'Delete', 'Space', 'ArrowUp', 'ArrowDown', 'ArrowLeft', 'ArrowRight', 'PageUp', 'PageDown',
  'Home', 'End', 'Shift+Tab', 'Control+A',
]);
const WAIT_UNTIL = new Set(['commit', 'domcontentloaded', 'load']);
const SAFE_SCHEMES = new Set(['http:', 'https:', 'data:', 'blob:', 'about:', 'ws:', 'wss:']);
const MAX_PAGES = 4;

for (const level of ['log', 'info', 'warn', 'error', 'debug']) {
  console[level] = (...a) => process.stderr.write(a.map(String).join(' ') + '\n');
}

class Fail extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

function clip(text, max) {
  const s = String(text == null ? '' : text);
  return s.length > max ? s.slice(0, max) : s;
}

function oneLine(text, max = 300) {
  return clip(String(text || '').replace(/\s+/g, ' ').trim(), max);
}

function send(object) {
  let line = JSON.stringify(object);
  if (Buffer.byteLength(line) > MAX_OUT) {
    line = JSON.stringify({ v: PROTOCOL, id: object.id, ok: false, error: { code: 'internal', message: 'the reply was too large' } });
  }
  process.stdout.write(line + '\n');
}

function int(value, name, min, max, fallback) {
  if (value === undefined || value === null) return fallback;
  if (!Number.isInteger(value) || value < min || value > max) throw new Fail('invalid', `${name} must be an integer from ${min} to ${max}`);
  return value;
}

function text(value, name, max, required = false) {
  if (value === undefined || value === null) {
    if (required) throw new Fail('invalid', `${name} is required`);
    return null;
  }
  if (typeof value !== 'string' || value.length > max) throw new Fail('invalid', `${name} must be text of at most ${max} characters`);
  return value;
}

const state = {
  browser: null,
  context: null,
  page: null,
  dir: null,
  navTimeout: 30000,
  actionTimeout: 10000,
  allowDownloads: false,
  downloads: [],
  shots: 0,
  downloadCount: 0,
  crashed: false,
  proxyDone: false,
};

function sessionDir() {
  const dir = process.env.BRIDLE_BROWSER_DIR;
  if (!dir || !path.isAbsolute(dir)) throw new Fail('internal', 'the session folder is not set');
  return dir;
}

function playwright() {
  const root = process.env.BRIDLE_PLAYWRIGHT_DIR;
  if (!root || !path.isAbsolute(root)) throw new Fail('internal', 'the Playwright folder is not set');
  return require(path.join(root, 'index.js'));
}

function playwrightVersion() {
  const root = process.env.BRIDLE_PLAYWRIGHT_DIR;
  return JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8')).version;
}

// ----------------------------------------------------------------------------------------------------------------- launch

// Chromium takes only the LAST --disable-features it is given, so a list of our own would silently replace Playwright's and bring back
// the connections that list exists to prevent (a time query to clients2.google.com, an "AI Mode" check to www.google.com, optimisation
// hints, translation, upgrades). So ours is Playwright's list for the pinned version (playwright-core 1.64.1,
// chromiumSwitches) plus Bridle's additions. The self-test fails if Chromium contacts anything on its own, which would catch drift
// when the pin moves.
const DISABLED_FEATURES = [
  // Playwright 1.64.1's own list.
  'AvoidUnnecessaryBeforeUnloadCheckSync', 'DestroyProfileOnBrowserClose', 'DialMediaRouteProvider', 'GlobalMediaControls', 'HttpsUpgrades',
  'LensOverlay', 'MediaRouter', 'PaintHolding', 'ThirdPartyStoragePartitioning', 'BlockOriginHeaderModificationOnRedirect',
  'AvoidCorsURLLoaderRestartOnRedirect', 'Translate', 'AutoDeElevate', 'OptimizationHints', 'NetworkTimeServiceQuerying', 'AimEnabled',
  // Bridle's additions. The three Preconnect ones stop Chromium opening a connection to the default search engine (www.google.com)
  // when a session starts; found with a net log (the connection was a preconnect started by the browser itself).
  'AutofillServerCommunication', 'DnsOverHttps', 'UseDnsHttpsSvcb', 'PreconnectToSearch', 'PreconnectToSearchDesktop',
  'SearchEnginePreconnect2',
];

const LAUNCH_ARGS = [
  '--disable-extensions',
  '--disable-component-update',
  '--disable-background-networking',
  '--disable-sync',
  '--disable-default-apps',
  '--disable-breakpad',
  '--disable-domain-reliability',
  '--disable-quic', // UDP does not go through a proxy
  '--disable-notifications',
  '--deny-permission-prompts',
  '--no-first-run',
  '--no-default-browser-check',
  '--mute-audio',
  '--disable-features=' + DISABLED_FEATURES.join(','),
  '--force-webrtc-ip-handling-policy=disable_non_proxied_udp',
  // Nothing may be looked up by name outside the proxy: the proxy's own address is a number.
  '--host-resolver-rules=MAP * ~NOTFOUND , EXCLUDE 127.0.0.1',
];

async function launch(args) {
  if (state.browser) throw new Fail('invalid', 'the browser is already running');
  const proxy = args.proxy || {};
  const server = text(proxy.server, 'proxy.server', 100, true);
  if (!/^http:\/\/127\.0\.0\.1:\d{1,5}$/.test(server)) throw new Fail('invalid', 'the proxy must be on 127.0.0.1');
  const username = text(proxy.username, 'proxy.username', 100, true);
  const password = text(proxy.password, 'proxy.password', 200, true);
  state.navTimeout = int(args.nav_timeout_ms, 'nav_timeout_ms', 1000, 120000, 30000);
  state.actionTimeout = int(args.action_timeout_ms, 'action_timeout_ms', 500, 60000, 10000);
  state.allowDownloads = args.downloads === true;
  const width = int(args.width, 'width', 320, 1920, 1280);
  const height = int(args.height, 'height', 240, 1200, 800);
  state.dir = sessionDir();
  // Playwright keeps its own copy of each download under a random name until the browser closes; ours are `downloads/dl-N.bin`.
  const downloadsPath = path.join(state.dir, 'pw-downloads');
  fs.mkdirSync(downloadsPath, { recursive: true });
  fs.mkdirSync(path.join(state.dir, 'downloads'), { recursive: true });
  fs.mkdirSync(path.join(state.dir, 'shots'), { recursive: true });

  const { chromium } = playwright();
  state.browser = await chromium.launch({
    channel: 'chromium', // the full Chromium build, not the old headless shell
    headless: true,
    chromiumSandbox: true, // Playwright turns the sandbox off unless told; Bridle keeps it on
    args: LAUNCH_ARGS,
    proxy: { server, username, password },
    downloadsPath,
    handleSIGINT: false,
    handleSIGTERM: false,
    handleSIGHUP: false,
    timeout: 30000,
  });
  state.browser.on('disconnected', () => {
    state.crashed = true;
  });
  state.context = await state.browser.newContext({
    viewport: { width, height },
    acceptDownloads: state.allowDownloads,
    serviceWorkers: 'block',
    permissions: [],
    javaScriptEnabled: true,
    ignoreHTTPSErrors: false,
    bypassCSP: false,
    locale: 'en-US',
    colorScheme: 'light',
  });
  state.context.setDefaultTimeout(state.actionTimeout);
  state.context.setDefaultNavigationTimeout(state.navTimeout);
  await state.context.route('**/*', async (route) => {
    let scheme = '';
    try {
      scheme = new URL(route.request().url()).protocol;
    } catch (_) {
      return route.abort('blockedbyclient');
    }
    if (!SAFE_SCHEMES.has(scheme)) return route.abort('blockedbyclient');
    return route.continue();
  });
  state.context.on('page', (page) => {
    if (state.context.pages().length > MAX_PAGES) {
      page.close().catch(() => {});
      return;
    }
    hook(page);
    state.page = page;
  });
  state.page = await state.context.newPage();
  return { browser_version: state.browser.version(), playwright_version: playwrightVersion() };
}

function hook(page) {
  page.on('crash', () => {
    state.crashed = true;
  });
  page.on('download', async (download) => {
    if (!state.allowDownloads) {
      await download.cancel().catch(() => {});
      return;
    }
    const n = ++state.downloadCount;
    const file = `dl-${n}.bin`;
    const entry = { file, suggested_name: oneLine(download.suggestedFilename(), 120), ready: false, error: null, bytes: 0, url: download.url() };
    state.downloads.push(entry);
    try {
      const target = path.join(state.dir, 'downloads', file);
      await download.saveAs(target);
      entry.bytes = fs.statSync(target).size;
      entry.ready = true;
    } catch (e) {
      entry.error = oneLine(e.message, 160);
      entry.ready = true;
    }
  });
}

// -------------------------------------------------------------------------------------------------------------- helpers

function live() {
  if (!state.browser || !state.page || state.crashed) throw new Fail('crashed', 'the browser is not running');
  return state.page;
}

async function title(page) {
  return oneLine(await page.title().catch(() => ''), 300);
}

function failure(e) {
  if (e instanceof Fail) return e;
  const message = oneLine(e && e.message, 300);
  if (/timeout/i.test(message) && /exceeded|waiting/i.test(message)) return new Fail('timeout', message);
  if (/net::ERR_|NS_ERROR|ERR_/.test(message)) return new Fail('navigation', message.replace(/\s*Call log:.*/i, ''));
  if (/crash|closed|disconnected|Target page/i.test(message)) return new Fail('crashed', message);
  return new Fail('internal', message);
}

// ---------------------------------------------------------------------------------------------------------------- navigate

async function navigate(args) {
  const page = live();
  const url = text(args.url, 'url', 2048, true);
  if (!/^https?:\/\//i.test(url)) throw new Fail('invalid', 'only http and https addresses are allowed');
  const waitUntil = args.wait_until || 'domcontentloaded';
  if (!WAIT_UNTIL.has(waitUntil)) throw new Fail('invalid', 'wait_until must be commit, domcontentloaded or load');
  const settle = int(args.settle_ms, 'settle_ms', 0, 5000, 2000);
  const response = await page.goto(url, { waitUntil, timeout: state.navTimeout });
  let redirects = 0;
  if (response) {
    for (let r = response.request(); r && r.redirectedFrom(); r = r.redirectedFrom()) redirects += 1;
  }
  if (settle > 0 && waitUntil !== 'load') {
    // Pages that build themselves with script need a moment after the document is parsed. Wait for the load event, but never
    // for "network idle": a page can keep a connection open forever.
    await page.waitForLoadState('load', { timeout: settle }).catch(() => {});
  }
  return {
    status: response ? response.status() : null,
    url: page.url(),
    title: await title(page),
    redirects,
    ready_state: await page.evaluate(() => document.readyState).catch(() => 'unknown'),
  };
}

// ---------------------------------------------------------------------------------------------------------------- snapshot

const META = () => {
  const pick = (selector, attr) => {
    const el = document.querySelector(selector);
    const value = el ? (attr ? el.getAttribute(attr) : el.textContent) : null;
    return value ? String(value).trim().slice(0, 400) : null;
  };
  return {
    description: pick('meta[name="description"]', 'content'),
    canonical: pick('link[rel="canonical"]', 'href'),
    lang: document.documentElement ? document.documentElement.getAttribute('lang') : null,
    robots: pick('meta[name="robots"]', 'content'),
    author: pick('meta[name="author"]', 'content'),
    og_title: pick('meta[property="og:title"]', 'content'),
    og_type: pick('meta[property="og:type"]', 'content'),
    published: pick('meta[property="article:published_time"]', 'content') || pick('meta[name="date"]', 'content'),
    frames: document.querySelectorAll('iframe, frame').length,
  };
};

// Waits until the page stops changing: no change to the document for `quiet` ms, at most `max` ms in all. A page that builds itself
// with script often does so a moment after the load event; "network idle" would wait forever on a page that keeps a connection open.
const SETTLE = ([quiet, max]) =>
  new Promise((resolve) => {
    let timer = null;
    const finish = () => {
      observer.disconnect();
      resolve(true);
    };
    const observer = new MutationObserver(() => {
      clearTimeout(timer);
      timer = setTimeout(finish, quiet);
    });
    observer.observe(document.documentElement || document, { subtree: true, childList: true, characterData: true, attributes: true });
    timer = setTimeout(finish, quiet);
    setTimeout(finish, max);
  });

async function snapshot(args) {
  const page = live();
  await page.evaluate(SETTLE, [300, 2500]).catch(() => {});
  const maxHtml = int(args.max_html_bytes, 'max_html_bytes', 10000, 4 * 1024 * 1024, 2 * 1024 * 1024);
  const maxAria = int(args.aria_chars, 'aria_chars', 0, 60000, 8000);
  let html = await page.content();
  let truncated = false;
  if (Buffer.byteLength(html) > maxHtml) {
    html = html.slice(0, Math.floor(maxHtml / 2));
    while (Buffer.byteLength(html) > maxHtml) html = html.slice(0, Math.floor(html.length * 0.9));
    truncated = true;
  }
  let aria = null;
  let ariaTruncated = false;
  if (maxAria > 0) {
    try {
      const raw = await page.locator('body').ariaSnapshot({ timeout: Math.min(state.actionTimeout, 5000) });
      ariaTruncated = raw.length > maxAria;
      aria = clip(raw, maxAria);
    } catch (_) {
      aria = null;
    }
  }
  const meta = await page.evaluate(META).catch(() => ({}));
  return {
    url: page.url(),
    title: await title(page),
    ready_state: await page.evaluate(() => document.readyState).catch(() => 'unknown'),
    html,
    html_truncated: truncated,
    aria,
    aria_truncated: ariaTruncated,
    meta,
  };
}

async function screenshot() {
  const page = live();
  const file = `shot-${++state.shots}.png`;
  const target = path.join(state.dir, 'shots', file);
  // The visible window only: a "full page" picture of an endless page has no useful size limit.
  await page.screenshot({ path: target, type: 'png', fullPage: false, timeout: state.actionTimeout });
  const viewport = page.viewportSize() || { width: 0, height: 0 };
  return { file, width: viewport.width, height: viewport.height, bytes: fs.statSync(target).size, url: page.url() };
}

// ------------------------------------------------------------------------------------------------------------------ action

function locator(page, target) {
  if (!target || typeof target !== 'object') throw new Fail('invalid', 'target is required for this action');
  const exact = target.exact === true;
  const name = text(target.name, 'target.name', 300);
  const kinds = ['role', 'label', 'placeholder', 'text'].filter((k) => target[k] !== undefined && target[k] !== null);
  if (kinds.length !== 1) throw new Fail('invalid', 'target needs exactly one of role, label, placeholder or text');
  if (target.role !== undefined && target.role !== null) {
    if (typeof target.role !== 'string' || !ROLES.has(target.role)) throw new Fail('invalid', 'target.role is not a role this tool supports');
    return page.getByRole(target.role, name ? { name, exact } : {});
  }
  if (target.label) return page.getByLabel(text(target.label, 'target.label', 300, true), { exact });
  if (target.placeholder) return page.getByPlaceholder(text(target.placeholder, 'target.placeholder', 300, true), { exact });
  return page.getByText(text(target.text, 'target.text', 300, true), { exact });
}

async function one(page, target, timeout) {
  const loc = locator(page, target);
  const index = target.index === undefined || target.index === null ? null : int(target.index, 'target.index', 0, 99, 0);
  const deadline = Date.now() + timeout;
  let count = 0;
  for (;;) {
    count = await loc.count();
    if (count > 0 || Date.now() >= deadline) break;
    await new Promise((r) => setTimeout(r, 100));
  }
  if (count === 0) throw new Fail('not_found', 'no element matches that target');
  if (index === null && count > 1) {
    const seen = [];
    for (let i = 0; i < Math.min(count, 5); i += 1) {
      const label = await loc.nth(i).evaluate((e) => (e.getAttribute('aria-label') || e.innerText || e.textContent || e.tagName).trim().slice(0, 60)).catch(() => '?');
      seen.push(`${i}: ${oneLine(label, 60)}`);
    }
    throw new Fail('ambiguous', `${count} elements match; add "index" (0 to ${count - 1}) or a more specific target. First matches: ${seen.join(' | ')}`);
  }
  if (index !== null && index >= count) throw new Fail('not_found', `index ${index} is out of range: ${count} elements match`);
  return index === null ? loc.first() : loc.nth(index);
}

async function afterAction(page, before, waitDownloadMs) {
  const popup = state.page !== page;
  const current = state.page;
  if (!popup) {
    await current.waitForLoadState('domcontentloaded', { timeout: 1500 }).catch(() => {});
  }
  let download = null;
  const until = Date.now() + waitDownloadMs;
  while (Date.now() < until && !state.downloads.some((d) => !d.reported)) {
    await new Promise((r) => setTimeout(r, 50));
  }
  const entry = state.downloads.find((d) => !d.reported);
  if (entry) {
    const settleBy = Date.now() + state.actionTimeout * 3;
    while (!entry.ready && Date.now() < settleBy) await new Promise((r) => setTimeout(r, 100));
    entry.reported = true;
    download = { file: entry.file, suggested_name: entry.suggested_name, bytes: entry.bytes, error: entry.error, ready: entry.ready, url: entry.url };
  }
  return {
    url: current.url(),
    title: await title(current),
    navigated: current.url() !== before,
    popup,
    download,
  };
}

async function action(args) {
  const page = live();
  const kind = text(args.kind, 'kind', 20, true);
  const timeout = int(args.timeout_ms, 'timeout_ms', 200, 60000, state.actionTimeout);
  const before = page.url();
  let waitDownload = 700;
  switch (kind) {
    case 'click': {
      const el = await one(page, args.target, timeout);
      await el.click({ timeout });
      break;
    }
    case 'fill': {
      const value = text(args.value, 'value', 2000, true);
      const el = await one(page, args.target, timeout);
      const type = await el.getAttribute('type', { timeout }).catch(() => null);
      if (type && type.toLowerCase() === 'password') throw new Fail('blocked', 'Bridle does not type into password fields');
      await el.fill(value, { timeout });
      waitDownload = 0;
      break;
    }
    case 'select': {
      const value = text(args.value, 'value', 300, true);
      const el = await one(page, args.target, timeout);
      try {
        await el.selectOption({ label: value }, { timeout: Math.min(timeout, 2000) });
      } catch (_) {
        await el.selectOption({ value }, { timeout });
      }
      waitDownload = 0;
      break;
    }
    case 'press': {
      const key = text(args.key, 'key', 20, true);
      if (!KEYS.has(key) && !/^[A-Za-z0-9]$/.test(key)) throw new Fail('invalid', 'that key is not supported');
      if (args.target) {
        const el = await one(page, args.target, timeout);
        await el.press(key, { timeout });
      } else {
        await page.keyboard.press(key);
      }
      break;
    }
    case 'scroll': {
      const direction = text(args.direction, 'direction', 10, true);
      const amount = int(args.amount, 'amount', 1, 5000, 600);
      if (!['up', 'down', 'top', 'bottom'].includes(direction)) throw new Fail('invalid', 'direction must be up, down, top or bottom');
      await page.evaluate(([d, a]) => {
        if (d === 'top') window.scrollTo(0, 0);
        else if (d === 'bottom') window.scrollTo(0, document.documentElement.scrollHeight);
        else window.scrollBy(0, d === 'down' ? a : -a);
      }, [direction, amount]);
      waitDownload = 0;
      break;
    }
    case 'wait': {
      const el = await one(page, args.target, timeout);
      await el.waitFor({ state: 'visible', timeout });
      waitDownload = 0;
      break;
    }
    case 'wait_download': {
      waitDownload = timeout;
      break;
    }
    case 'back':
    case 'forward': {
      const go = kind === 'back' ? page.goBack.bind(page) : page.goForward.bind(page);
      const response = await go({ waitUntil: 'domcontentloaded', timeout: state.navTimeout });
      if (!response && page.url() === before) {
        return { ...(await afterAction(page, before, 0)), note: `there is nothing to go ${kind} to` };
      }
      waitDownload = 0;
      break;
    }
    default:
      throw new Fail('invalid', `unknown action ${oneLine(kind, 20)}`);
  }
  return afterAction(page, before, waitDownload);
}

async function close() {
  const browser = state.browser;
  state.browser = null;
  state.page = null;
  if (browser) await browser.close().catch(() => {});
  return {};
}

// -------------------------------------------------------------------------------------------------------------------- loop

const HANDLERS = { launch, navigate, snapshot, action, screenshot, close };

async function handle(line) {
  let request;
  try {
    request = JSON.parse(line);
  } catch (_) {
    return send({ v: PROTOCOL, id: null, ok: false, error: { code: 'invalid', message: 'not JSON' } });
  }
  const id = Number.isInteger(request.id) ? request.id : null;
  try {
    if (request.v !== PROTOCOL) throw new Fail('invalid', `protocol ${request.v} is not supported`);
    const op = HANDLERS[request.op];
    if (!Object.prototype.hasOwnProperty.call(HANDLERS, request.op) || !op) throw new Fail('invalid', 'unknown operation');
    const result = await op(request.args || {});
    send({ v: PROTOCOL, id, ok: true, result });
    if (request.op === 'close') process.exit(0);
  } catch (e) {
    const f = failure(e);
    send({ v: PROTOCOL, id, ok: false, error: { code: f.code, message: f.message } });
  }
}

async function main() {
  if (process.argv[2] === '--probe') {
    // For `bridle web browser setup` and `status`: say which Playwright this is and where it keeps Chromium. Nothing is launched.
    const { chromium } = playwright();
    process.stdout.write(JSON.stringify({ playwright_version: playwrightVersion(), executable: chromium.executablePath() }) + '\n');
    return;
  }
  process.stdout.write(JSON.stringify({ v: PROTOCOL, event: 'ready', playwright_version: playwrightVersion() }) + '\n');
  const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  let chain = Promise.resolve();
  rl.on('line', (line) => {
    chain = chain.then(() => handle(line));
  });
  rl.on('close', async () => {
    // The Rust side is gone or closed the pipe: free Chromium and leave.
    await chain.catch(() => {});
    await close();
    process.exit(0);
  });
}

process.on('uncaughtException', (e) => {
  process.stderr.write(`worker error: ${oneLine(e && e.message)}\n`);
  process.exit(1);
});
process.on('unhandledRejection', (e) => {
  process.stderr.write(`worker rejection: ${oneLine(e && e.message)}\n`);
});

main().catch((e) => {
  process.stderr.write(`worker failed: ${oneLine(e && e.message)}\n`);
  process.exit(1);
});
