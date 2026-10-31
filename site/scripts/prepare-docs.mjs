// Builds the documentation site's pages. Run before `astro dev` and `astro build`.
//
// Sources are the Markdown files in the repository. Nothing in them is changed on disk: for each published page the
// script writes a copy under src/content/docs/ (git-ignored) with a frontmatter title and description, an edit link,
// and links rewritten for the site. The home page is site/home/index.mdx and is copied as it is.
//
// Links to a published page become that page's route. Links to any other file or folder in the repository become
// GitHub URLs. A link whose target does not exist is left as written, and the build fails.
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { BASE } from '../site.config.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const SITE = path.resolve(here, '..');
const ROOT = path.resolve(SITE, '..');
const OUT = path.join(SITE, 'src', 'content', 'docs');
const HOME = path.join(SITE, 'home', 'index.mdx');
const REPO = 'https://github.com/kushagrathisside/bridle';

// Published pages: repository-relative source, site route, and the description for search engines and link previews.
const PAGES = [
  { source: 'docs/overview/introduction.md', route: 'introduction', description: 'What Bridle is, who it is for, and what it does to a coding agent\'s tool calls.' },
  { source: 'docs/overview/getting-started.md', route: 'getting-started', description: 'Build Bridle, set up a repository, and see a first policy decision, step by step.' },
  { source: 'docs/examples/index.md', route: 'examples', description: 'Worked examples that show Bridle on a small repository.' },
  { source: 'docs/examples/fix-a-failing-test.md', route: 'examples/fix-a-failing-test', description: 'Fix a failing unit test with Bridle, from the first command to the passing run.' },
  { source: 'docs/guide/installation.md', route: 'guides/install', description: 'Prerequisites and build steps for Bridle on Windows, and how to write the machine configuration.' },
  { source: 'docs/guide/set-up-a-repository.md', route: 'guides/set-up-a-repository', description: 'Add Bridle to a repository: profiles, generated files, and validating the configuration.' },
  { source: 'docs/guide/run-verification.md', route: 'guides/run-verification', description: 'Declare your test, lint and type-check commands so the agent can run them through Bridle.' },
  { source: 'docs/guide/models.md', route: 'guides/serve-models', description: 'Download the model files, check their checksums, and start and check the local model servers.' },
  { source: 'docs/guide/run-a-task.md', route: 'guides/run-a-task', description: 'Start, inspect, resume and reset a Bridle task, and read the decisions it recorded.' },
  { source: 'docs/guide/troubleshooting.md', route: 'guides/troubleshoot', description: 'Fixes for the problems people hit most when setting up Bridle on Windows.' },
  { source: 'docs/concepts/how-decisions-are-made.md', route: 'concepts/how-decisions-are-made', description: 'How Bridle decides whether each tool call runs, is blocked, or is refused, and why.' },
  { source: 'docs/concepts/tasks-and-loops.md', route: 'concepts/tasks-and-loops', description: 'Tasks, the journal that records them, and the loop rules that refuse repeated calls.' },
  { source: 'docs/concepts/models.md', route: 'concepts/models-and-slots', description: 'The model slots Bridle uses, what each one does, and which ones are optional.' },
  { source: 'docs/reference/cli.md', route: 'reference/cli', description: 'Every agent command and its options.' },
  { source: 'docs/guide/configuration.md', route: 'reference/configuration', description: 'Configuration files, profiles, keys with their defaults, and the validation rules.' },
  { source: 'docs/reference/mcp-tools.md', route: 'reference/mcp-tools', description: 'The tools Bridle serves to the agent over MCP, with the policy class of each.' },
  { source: 'docs/reference/modules/README.md', route: 'reference/developer', title: 'Developer reference', description: 'Module-level reference for the Bridle crates, for people changing the code.' },
  { source: 'docs/project/principles.md', route: 'project/principles', description: 'The design principles behind Bridle\'s policy, its state, and its boundaries.' },
  { source: 'docs/concepts/architecture.md', route: 'project/architecture', description: 'How Bridle is layered, what happens to one tool call, and where its state is kept.' },
  { source: 'docs/concepts/security.md', route: 'project/security-model', description: 'What Bridle protects, the rules it applies to each tool call, and the risks that remain.' },
  { source: 'docs/project/status.md', route: 'project/status', description: 'What is stable, what is optional or experimental, what is not live yet, and the known limits.' },
  { source: 'CONTRIBUTING.md', route: 'project/contributing', description: 'How to build, test and change Bridle, and what a change must pass before it is merged.' },
  { source: 'CHANGELOG.md', route: 'project/changelog', description: 'What changed in each version of Bridle.' },
  { source: 'SECURITY.md', route: 'project/security-policy', description: 'How to report a security problem in Bridle privately.' },
];

// Every module page is published under the developer reference, and every decision record under the project section.
const MODULE_DIR = 'docs/reference/modules';
const DECISION_DIR = 'docs/decisions';
const moduleEntries = fs.readdirSync(path.join(ROOT, MODULE_DIR)).filter((f) => f.endsWith('.md') && f !== 'README.md');
const decisionEntries = fs.readdirSync(path.join(ROOT, DECISION_DIR)).filter((f) => f.endsWith('.md'));
for (const file of moduleEntries) {
  PAGES.push({ source: `${MODULE_DIR}/${file}`, route: `reference/developer/${file.replace(/\.md$/, '')}` });
}
for (const file of decisionEntries) {
  PAGES.push({ source: `${DECISION_DIR}/${file}`, route: `project/decisions/${file.replace(/\.md$/, '').replace(/^(\d{4})-/, 'adr-$1-')}` });
}

const ROUTE_OF = new Map(PAGES.map((page) => [page.source, page.route]));
const EXTERNAL = /^[a-z][a-z0-9+.-]*:/i;
const FENCE = /^\s*(```|~~~)/;
const HEADING = /^#\s+(.+?)\s*#*\s*$/;
const DEFINITION = /^(\s{0,3}\[[^\]]+\]:\s*)(\S+)(.*)$/;
const LINK = /(!?)\[((?:[^\[\]]|\[[^\[\]]*\])*)\]\(([^)\s]+)((?:\s+"[^"]*")?)\)/g;
const stats = { pages: 0, toPages: 0, toGitHub: 0, broken: 0 };

function routeHref(route) {
  return route === '' ? `${BASE}/` : `${BASE}/${route}/`;
}

// The repository-relative path of a target, or null when it lies outside the repository.
function repoPath(absolute) {
  const rel = path.relative(ROOT, absolute).split(path.sep).join('/');
  return rel.startsWith('..') ? null : rel;
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
  const rel = repoPath(target);
  if (rel !== null && fs.statSync(target).isDirectory()) {
    const index = `${rel}/README.md`;
    if (ROUTE_OF.has(index)) {
      stats.toPages += 1;
      return routeHref(ROUTE_OF.get(index)) + hash;
    }
  } else if (rel !== null && ROUTE_OF.has(rel)) {
    stats.toPages += 1;
    return routeHref(ROUTE_OF.get(rel)) + hash;
  }
  stats.toGitHub += 1;
  const kind = fs.statSync(target).isDirectory() ? 'tree' : 'blob';
  return `${REPO}/${kind}/main/${rel ?? decoded}${hash}`;
}

function rewriteInline(line, fromDir, page) {
  return line
    .split(/(`[^`]*`)/)
    .map((part, index) =>
      index % 2 === 1
        ? part
        : part.replace(LINK, (match, bang, text, url, title) => `${bang}[${text}](${retarget(url, fromDir, page)}${title})`),
    )
    .join('');
}

// Splits a page into its first top-level heading (the title) and the rest, rewriting links on the way.
function transform(source, fromDir, page) {
  let inFence = false;
  let title;
  const body = [];
  for (const line of source.split('\n')) {
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
  return { title, body: body.join('\n').replace(/^\n+/, '') };
}

// The first paragraph of a page as plain text, shortened for a description.
function summary(body) {
  let inFence = false;
  for (const raw of body.split('\n')) {
    const line = raw.trim();
    if (FENCE.test(raw)) {
      inFence = !inFence;
      continue;
    }
    if (inFence || !line || line.startsWith('#') || line.startsWith('|') || line.startsWith('>') || line.startsWith('-')) continue;
    const text = line.replace(/`([^`]*)`/g, '$1').replace(/\[([^\]]*)\]\([^)]*\)/g, '$1').replace(/\*\*/g, '').trim();
    return text.length > 155 ? `${text.slice(0, 152).trimEnd()}...` : text;
  }
  return '';
}

function main() {
  for (const page of PAGES) {
    if (!fs.existsSync(path.join(ROOT, page.source))) throw new Error(`missing source: ${page.source}`);
  }
  fs.rmSync(OUT, { recursive: true, force: true });
  for (const page of PAGES) {
    const file = path.join(ROOT, page.source);
    const { title, body } = transform(fs.readFileSync(file, 'utf8'), path.dirname(file), page.source);
    const resolvedTitle = page.title ?? title ?? path.basename(page.source, '.md');
    const description = page.description ?? summary(body);
    const frontmatter = [
      '---',
      `title: ${JSON.stringify(resolvedTitle)}`,
      `description: ${JSON.stringify(description)}`,
      `editUrl: ${JSON.stringify(`${REPO}/edit/main/${page.source}`)}`,
      '---',
      '',
    ].join('\n');
    const target = path.join(OUT, `${page.route}.md`);
    fs.mkdirSync(path.dirname(target), { recursive: true });
    fs.writeFileSync(target, frontmatter + body);
    stats.pages += 1;
  }
  fs.mkdirSync(OUT, { recursive: true });
  fs.copyFileSync(HOME, path.join(OUT, 'index.mdx'));
  console.log(
    `docs: ${stats.pages} pages written; links to pages ${stats.toPages}, to GitHub ${stats.toGitHub}, broken ${stats.broken}`,
  );
  if (stats.broken > 0) process.exitCode = 1;
}

main();
