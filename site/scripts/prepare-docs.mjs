// Builds the site's pages from docs/. Run before `astro dev` and `astro build`.
//
// The Markdown in docs/ is not changed. For each published page this script writes a copy under
// src/content/docs/ (git-ignored) with:
//   - a frontmatter title, taken from the page's first top-level heading, which is then removed from the body
//     (Starlight shows the title itself); a page with no heading takes its file name;
//   - an editUrl that points at the page's file on GitHub;
//   - links rewritten for the site: a link to a published page becomes that page's route, and a link to any other
//     file or folder in the repository becomes its GitHub URL, so it still opens.
// Links inside fenced code and inline code are left as written. A link whose target does not exist is left as
// written and reported.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { BASE } from '../site.config.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const SITE = path.resolve(here, '..');
const DOCS = path.resolve(SITE, '..', 'docs');
const ROOT = path.resolve(DOCS, '..');
const OUT = path.join(SITE, 'src', 'content', 'docs');
const REPO = 'https://github.com/kushagrathisside/bridle';

// Paths under docs/ that stay on GitHub and are not published on the site: the status records (the progress log,
// the milestone records and the checklist), the project brief, the requirements register and the archive.
// A folder ends with "/".
const UNPUBLISHED = ['archive/', 'status/', 'spec/', 'reference/requirements-traceability.md'];

const EXTERNAL = /^[a-z][a-z0-9+.-]*:/i;
const FENCE = /^\s*(```|~~~)/;
const HEADING = /^#\s+(.+?)\s*#*\s*$/;
const DEFINITION = /^(\s{0,3}\[[^\]]+\]:\s*)(\S+)(.*)$/;
const LINK = /(!?)\[((?:[^\[\]]|\[[^\[\]]*\])*)\]\(([^)\s]+)((?:\s+"[^"]*")?)\)/g;

const stats = { pages: 0, toPages: 0, toGitHub: 0, broken: 0 };

// The site route of a page. `entry` is relative to docs/ with forward slashes, for example "guide/configuration.md".
function routeFor(entry) {
  const page = entry.replace(/\.md$/, '');
  if (page === 'README') return '/';
  if (page.endsWith('/README')) return `/${page.slice(0, -'/README'.length)}/`;
  return `/${page}/`;
}

function isPublic(entry) {
  return !UNPUBLISHED.some((item) => (item.endsWith('/') ? entry.startsWith(item) : entry === item));
}

// The file a route is written to, under src/content/docs/.
function outputFor(entry) {
  const route = routeFor(entry);
  return route === '/' ? 'index.md' : `${route.slice(1, -1)}.md`;
}

function listMarkdown(dir, base = '') {
  const found = [];
  for (const item of fs.readdirSync(dir, { withFileTypes: true })) {
    const rel = base ? `${base}/${item.name}` : item.name;
    if (item.isDirectory()) found.push(...listMarkdown(path.join(dir, item.name), rel));
    else if (item.name.endsWith('.md')) found.push(rel);
  }
  return found;
}

function retarget(url, fromDir, page) {
  if (!url || EXTERNAL.test(url) || url.startsWith('#') || url.startsWith('/')) return url;
  const hashAt = url.indexOf('#');
  const pathPart = hashAt === -1 ? url : url.slice(0, hashAt);
  const hash = hashAt === -1 ? '' : url.slice(hashAt);
  if (!pathPart) return url;

  let decoded = pathPart;
  try {
    decoded = decodeURI(pathPart);
  } catch {
    // Keep the path as written when it is not valid percent-encoding.
  }
  const target = path.resolve(fromDir, decoded);
  if (!fs.existsSync(target)) {
    stats.broken += 1;
    console.warn(`docs: broken link in ${page}: ${url}`);
    return url;
  }
  const entry = path.relative(DOCS, target).split(path.sep).join('/');
  if (!entry.startsWith('..') && entry.endsWith('.md') && isPublic(entry)) {
    stats.toPages += 1;
    return BASE + routeFor(entry) + hash;
  }
  stats.toGitHub += 1;
  const repoPath = path.relative(ROOT, target).split(path.sep).join('/');
  const kind = fs.statSync(target).isDirectory() ? 'tree' : 'blob';
  return `${REPO}/${kind}/main/${repoPath}${hash}`;
}

function rewriteInline(line, fromDir, page) {
  // Odd parts are inline code spans; they are kept as written.
  return line
    .split(/(`[^`]*`)/)
    .map((part, index) =>
      index % 2 === 1
        ? part
        : part.replace(LINK, (match, bang, text, url, title) => `${bang}[${text}](${retarget(url, fromDir, page)}${title})`),
    )
    .join('');
}

function transform(source, fromDir, page) {
  const lines = source.split('\n');
  let inFence = false;
  let title;
  const body = [];
  for (const line of lines) {
    if (FENCE.test(line)) {
      inFence = !inFence;
      body.push(line);
      continue;
    }
    if (inFence) {
      body.push(line);
      continue;
    }
    if (title === undefined) {
      const heading = line.match(HEADING);
      if (heading) {
        title = heading[1].trim();
        continue;
      }
    }
    const definition = line.match(DEFINITION);
    if (definition) {
      body.push(definition[1] + retarget(definition[2], fromDir, page) + definition[3]);
      continue;
    }
    body.push(rewriteInline(line, fromDir, page));
  }
  return { title, body: body.join('\n') };
}

function main() {
  fs.rmSync(OUT, { recursive: true, force: true });
  const pages = listMarkdown(DOCS).filter(isPublic);
  for (const entry of pages) {
    const page = path.join(DOCS, entry);
    const source = fs.readFileSync(page, 'utf8');
    const fromDir = path.dirname(page);
    const { title, body } = transform(source, fromDir, entry);
    const fallback = path.basename(entry, '.md');
    const frontmatter = [
      '---',
      `title: ${JSON.stringify(title ?? fallback)}`,
      `editUrl: ${JSON.stringify(`${REPO}/edit/main/docs/${entry}`)}`,
      '---',
      '',
    ].join('\n');
    const target = path.join(OUT, outputFor(entry));
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, frontmatter + body);
    stats.pages += 1;
  }
  console.log(
    `docs: ${stats.pages} pages written; links to pages ${stats.toPages}, to GitHub ${stats.toGitHub}, broken ${stats.broken}`,
  );
  if (stats.broken > 0) process.exitCode = 1;
}

main();
