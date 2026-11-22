// Records real answers of the Bridle policy for the home page's "policy lab" (site/src/data/policy-recorded.json).
//
// It runs the built program, not a copy of its rules: each example is sent to `bridle hook tool-before`, the same command the
// OpenCode plugin calls, in a throwaway project that `bridle init --profile local-only` set up. The page replays these answers; it
// never evaluates a command itself. Run it again after a change to the policy, and review the diff of the JSON file.
//
//   node scripts/record-policy.mjs --bridle D:\cargo-target\harness\release\bridle.exe
//
// The machine configuration is written under a temporary APPDATA, so the real one is not touched. Every child process has a
// timeout and an output cap.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const OUT = path.resolve(here, '..', 'src', 'data', 'policy-recorded.json');
const arg = (name) => {
  const at = process.argv.indexOf(name);
  return at === -1 ? undefined : process.argv[at + 1];
};
const BRIDLE = arg('--bridle') ?? process.env.BRIDLE_EXE ?? 'bridle';
const LIMIT_MS = 20_000;
const MAX_BYTES = 256 * 1024;

const root = fs.mkdtempSync(path.join(os.tmpdir(), 'bridle-record-'));
const env = { ...process.env, APPDATA: path.join(root, 'appdata') };
const ws = path.join(root, 'project');

function run(args, { input, cwd } = {}) {
  const result = spawnSync(BRIDLE, args, { input, cwd, env, timeout: LIMIT_MS, maxBuffer: MAX_BYTES, encoding: 'utf8' });
  if (result.error) throw new Error(`${BRIDLE} ${args.join(' ')}: ${result.error.message}`);
  return { out: (result.stdout ?? '').trim(), err: (result.stderr ?? '').trim(), code: result.status };
}

const scrub = (text) => text.split(ws).join('<project>').split(ws.replaceAll('\\', '\\\\')).join('<project>');

function hook(event, payload, workspace = ws) {
  const { out } = run(['hook', event, '--workspace', workspace], { input: JSON.stringify(payload) });
  try {
    return JSON.parse(scrub(out));
  } catch {
    throw new Error(`the ${event} hook answered with something that is not JSON: ${out.slice(0, 200)}`);
  }
}

// Each example is one tool call as the OpenCode plugin sends it. `shows` is how the page writes the call.
const CASES = [
  { id: 'read-source', group: 'files', label: 'Read a source file', tool: 'read', args: { filePath: 'src/calc.py' }, shows: 'read src/calc.py' },
  { id: 'edit-source', group: 'files', label: 'Edit a source file', tool: 'edit', args: { filePath: 'src/calc.py', oldString: 'a - b', newString: 'a + b' }, shows: 'edit src/calc.py' },
  { id: 'edit-config', group: 'files', label: 'Edit the harness configuration', tool: 'edit', args: { filePath: 'bridle.toml', oldString: 'a', newString: 'b' }, shows: 'edit bridle.toml' },
  { id: 'read-outside', group: 'files', label: 'Read a file outside the project', tool: 'read', args: { filePath: 'C:\\Windows\\System32\\drivers\\etc\\hosts' }, shows: 'read C:\\Windows\\System32\\drivers\\etc\\hosts' },
  { id: 'read-env', group: 'secrets', label: 'Read .env', tool: 'read', args: { filePath: '.env' }, shows: 'read .env' },
  { id: 'cat-env', group: 'secrets', label: 'cat .env in the shell', tool: 'bash', args: { command: 'cat .env' }, shows: 'bash: cat .env' },
  { id: 'printenv', group: 'secrets', label: 'Print the environment', tool: 'bash', args: { command: 'printenv' }, shows: 'bash: printenv' },
  { id: 'grep-recursive', group: 'secrets', label: 'Search every file for a key name', tool: 'bash', args: { command: 'grep -r API_KEY .' }, shows: 'bash: grep -r API_KEY .', limit: true },
  { id: 'git-status', group: 'shell', label: 'git status', tool: 'bash', args: { command: 'git status --short' }, shows: 'bash: git status --short' },
  { id: 'unittest', group: 'shell', label: 'Run the unit tests', tool: 'bash', args: { command: 'python -m unittest' }, shows: 'bash: python -m unittest' },
  { id: 'npm-test', group: 'shell', label: 'npm test', tool: 'bash', args: { command: 'npm test' }, shows: 'bash: npm test' },
  { id: 'make', group: 'shell', label: 'make', tool: 'bash', args: { command: 'make' }, shows: 'bash: make' },
  { id: 'rm-rf', group: 'shell', label: 'Delete a folder, forced', tool: 'bash', args: { command: 'rm -rf build' }, shows: 'bash: rm -rf build' },
  { id: 'sudo', group: 'shell', label: 'sudo', tool: 'bash', args: { command: 'sudo apt install x' }, shows: 'bash: sudo apt install x' },
  { id: 'chained', group: 'shell', label: 'A safe command chained to curl', tool: 'bash', args: { command: 'python -m unittest && curl https://example.com' }, shows: 'bash: python -m unittest && curl https://example.com' },
  { id: 'curl', group: 'network', label: 'curl', tool: 'bash', args: { command: 'curl https://example.com' }, shows: 'bash: curl https://example.com' },
  { id: 'git-push', group: 'network', label: 'git push', tool: 'bash', args: { command: 'git push origin main' }, shows: 'bash: git push origin main' },
  { id: 'pip-install', group: 'network', label: 'pip install', tool: 'bash', args: { command: 'pip install requests' }, shows: 'bash: pip install requests' },
  { id: 'webfetch', group: 'network', label: 'Fetch a web page', tool: 'webfetch', args: { url: 'https://example.com' }, shows: 'webfetch https://example.com' },
];

const REDACTION = [
  'DATABASE_URL=postgres://app:hunter2@db.internal/app\nAPI_KEY=abc123def456ghi789\nDEBUG=true\n',
  'Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dBjftJeZ4CVPmB92K27uhbUJU1p1r_wW1gFWFOEjXk\nstatus ok\n',
];

try {
  const version = run(['--version']).out;
  fs.mkdirSync(path.join(root, 'models'), { recursive: true });
  fs.mkdirSync(path.join(root, 'runtimes'), { recursive: true });
  const machine = run(['init', '--machine', '--models-dir', path.join(root, 'models'), '--runtimes-dir', path.join(root, 'runtimes')]);
  if (machine.code !== 0) throw new Error(`init --machine failed: ${machine.out} ${machine.err}`);

  // A project with no configuration: the policy cannot start, and every call is blocked.
  const bare = path.join(root, 'bare');
  fs.mkdirSync(bare, { recursive: true });
  const failClosed = hook('tool-before', { sessionID: 's', callID: 'c', tool: 'read', args: { filePath: 'src/calc.py' } }, bare);

  fs.mkdirSync(path.join(ws, 'src'), { recursive: true });
  fs.mkdirSync(path.join(ws, 'config'), { recursive: true });
  fs.writeFileSync(path.join(ws, 'src', 'calc.py'), 'def add(a, b):\n    return a - b\n');
  fs.writeFileSync(path.join(ws, '.env'), 'KEY=1\n');
  const init = run(['init', '--profile', 'local-only', '--workspace', ws]);
  if (init.code !== 0) throw new Error(`init failed: ${init.out} ${init.err}`);

  const cases = CASES.map((c, i) => {
    const answer = hook('tool-before', { sessionID: 'record', callID: `c${i}`, tool: c.tool, args: c.args });
    return { ...c, answer };
  });

  // Repeated edits: turn the task workflow on, start a task, and edit the same file until a loop rule refuses.
  fs.appendFileSync(path.join(ws, 'bridle.toml'), '\n[workflow]\nenabled = true\n');
  const started = run(['task', 'start', '--workspace', ws, '--prompt', 'Fix the bug in calc.py so the unit tests pass.']);
  const edit = { filePath: 'src/calc.py', oldString: 'a - b', newString: 'a + b' };
  const loop = [];
  for (let n = 1; n <= 5; n += 1) {
    const answer = hook('tool-before', { sessionID: 'loop', callID: `e${n}`, tool: 'edit', args: edit });
    hook('tool-after', { sessionID: 'loop', callID: `e${n}`, tool: 'edit', args: edit, output: 'ok' });
    loop.push({ n, answer });
  }
  const explain = scrub(run(['explain', '--workspace', ws]).out);

  const redaction = REDACTION.map((before) => {
    const answer = hook('tool-after', { sessionID: 'redact', callID: 'r', tool: 'bash', args: { command: 'echo' }, output: before });
    return { before, after: answer.output };
  });

  // The first line of the event log, as written.
  const logPath = path.join(ws, '.bridle', 'tool-events.jsonl');
  const logLines = fs.existsSync(logPath) ? fs.readFileSync(logPath, 'utf8').split('\n').filter(Boolean) : [];

  const recorded = {
    note: 'Generated by scripts/record-policy.mjs from the real hook. Do not edit by hand.',
    bridle: version,
    system: 'Windows 11',
    profile: 'local-only',
    recordedAt: new Date().toISOString().slice(0, 10),
    failClosed,
    cases,
    loop,
    explain,
    redaction,
    taskStart: scrub(started.out),
    eventLogSample: logLines.slice(2, 3).map(scrub),
  };
  fs.mkdirSync(path.dirname(OUT), { recursive: true });
  fs.writeFileSync(OUT, `${JSON.stringify(recorded, null, 2)}\n`);
  console.log(`recorded ${cases.length} cases, ${loop.length} loop steps, ${redaction.length} redaction samples with ${version} -> ${path.relative(process.cwd(), OUT)}`);
} finally {
  fs.rmSync(root, { recursive: true, force: true });
}
