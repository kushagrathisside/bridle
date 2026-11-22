// The stages of one tool call, for the architecture diagram. Every statement here is taken from docs/concepts/architecture.md,
// docs/concepts/security.md, ADR-0017 and the code paths named in `code`.
import { doc, links } from './site';

export type StageId = 'model' | 'host' | 'policy' | 'tool' | 'result' | 'log';

export interface Stage {
  id: StageId;
  /** The number shown on the stage. */
  step: number;
  name: string;
  owner: string;
  tone: 'outside' | 'bridle' | 'machine';
  /** A phrase of a few words under the name, on the diagram. */
  line: string;
  summary: string;
  owns: string[];
  /** What this stage is not responsible for. */
  not: string;
  /** What happens when this stage cannot do its job. */
  fails: string;
  code: { label: string; href: string }[];
  more: { label: string; href: string };
}

export const stages: Stage[] = [
  {
    id: 'model',
    step: 1,
    name: 'Model',
    owner: 'llama.cpp',
    tone: 'outside',
    line: 'proposes a tool call',
    summary:
      'A llama.cpp server on 127.0.0.1, one per model slot. It writes text and asks for tool calls. What it asks for is a proposal, not a permission.',
    owns: [
      'Generating the answer and the requested tool calls.',
      'Slots: main (Qwen3.5-9B, GPU), micro (LFM2.5-1.2B, CPU, optional) and deep (Ternary Bonsai 2 27B, on demand, off by default).',
    ],
    not: 'It decides nothing about what may run. Nothing it writes can change the policy.',
    fails:
      'A model endpoint that is not on this machine is rejected by the configuration, and loopback traffic never uses a proxy. A GPU server that cannot fit is refused before it loads, with the numbers named (at least 768 MiB must stay free).',
    code: [
      { label: 'providers/llama_cpp.rs', href: links.source('crates/harness-runtime/src/providers/llama_cpp.rs') },
      { label: 'vram.rs', href: links.source('crates/harness-runtime/src/vram.rs') },
    ],
    more: { label: 'Models and slots', href: doc('concepts/models-and-slots') },
  },
  {
    id: 'host',
    step: 2,
    name: 'Agent loop',
    owner: 'Bridle window and TUI, or OpenCode',
    tone: 'bridle',
    line: 'receives the call, asks first',
    summary:
      'The host runs the conversation. In the Bridle window and in `bridle tui` the loop is Bridle’s own. With OpenCode, OpenCode owns sessions and the loop, and a thin TypeScript plugin forwards each hook to `bridle hook`.',
    owns: [
      'Sessions, the conversation and the interface.',
      'Asking the policy about a call before acting on it. The OpenCode plugin holds no policy logic.',
      'At most 30 model calls in one turn.',
    ],
    not: 'No host can skip the decision path. The window has no file, shell or network access of its own except through its commands.',
    fails: 'If the hook cannot answer, the call is blocked. The plugin throws unless the verdict is allow.',
    code: [
      { label: 'chat.rs', href: links.source('crates/harness-cli/src/chat.rs') },
      { label: 'host/opencode_v1.rs', href: links.source('crates/harness-cli/src/host/opencode_v1.rs') },
      { label: 'opencode-plugin/bridle.ts', href: links.source('opencode-plugin/bridle.ts') },
    ],
    more: { label: 'ADR-0017: the window over the same loop', href: doc('project/decisions/adr-0017-desktop-app-over-the-agent-loop') },
  },
  {
    id: 'policy',
    step: 3,
    name: 'Policy engine',
    owner: 'Bridle',
    tone: 'bridle',
    line: 'judges the real arguments',
    summary:
      'A deterministic policy in a crate with no network dependencies. It checks secrets, paths and protected files, classifies a shell command by its program and arguments, and returns allow, ask or deny. With the task workflow on, loop rules read the journal as well.',
    owns: [
      'Seven command classes, READ_ONLY up to PRIVILEGED, each with a configured verdict.',
      '`ask` is never allowed: it is reported as a block, because nobody is there to approve it.',
      'Loop rules: repeated edit, repeated command, same failure, implementation retries, iteration limit.',
    ],
    not: 'It does not run anything and does not isolate a process that is set on bypassing it.',
    fails:
      'Fails closed: a command that cannot be parsed is denied, an unrecognised tool is treated as risky and blocked, and a configuration with errors blocks every call.',
    code: [
      { label: 'tool_policy.rs', href: links.source('crates/harness-core/src/security/tool_policy.rs') },
      { label: 'command_risk.rs', href: links.source('crates/harness-core/src/security/command_risk.rs') },
      { label: 'path_guard.rs', href: links.source('crates/harness-core/src/security/path_guard.rs') },
      { label: 'loops.rs', href: links.source('crates/harness-core/src/loops.rs') },
    ],
    more: { label: 'How decisions are made', href: doc('concepts/how-decisions-are-made') },
  },
  {
    id: 'tool',
    step: 4,
    name: 'Tool execution',
    owner: 'Bridle tools, under your account',
    tone: 'machine',
    line: 'runs what was allowed',
    summary:
      'An allowed call runs: file tools with atomic writes, Git, tree-sitter navigation, the verification runners and `run_command`. Web research and the browser exist but are off by default.',
    owns: [
      'Running the allowed call, bounded in time and output.',
      'Verification steps come from the project configuration and are classified by the same command rules first.',
    ],
    not: 'This stage is not isolated. An allowed command runs with your user’s rights, and on Windows Bridle is not a sandbox.',
    fails: 'A tool that is not on the policy’s lists is refused. A repository cannot add tools; adapters are declared in the machine configuration.',
    code: [
      { label: 'mcp/tools/', href: links.tree('crates/harness-runtime/src/mcp/tools') },
      { label: 'mcp/mod.rs', href: links.source('crates/harness-runtime/src/mcp/mod.rs') },
    ],
    more: { label: 'MCP tools', href: doc('reference/mcp-tools') },
  },
  {
    id: 'result',
    step: 5,
    name: 'Result processing',
    owner: 'Bridle',
    tone: 'bridle',
    line: 'redacts before the model reads',
    summary:
      'Output is bounded and redacted before it goes back to the model. Known secret shapes and KEY=value assignments are replaced with [REDACTED]. Older tool results are shortened in each request; the stored session is not changed.',
    owns: [
      'Redaction of tool output, of the event log and of `bridle config show`.',
      'Secret-looking environment variables are blanked for shell tools.',
    ],
    not: 'Redaction is a control, not a guarantee. A secret in a format it does not recognise reaches the model.',
    fails: 'If redaction cannot run, the output is withheld and the model gets a notice.',
    code: [
      { label: 'redact.rs', href: links.source('crates/harness-core/src/security/redact.rs') },
      { label: 'context.rs', href: links.source('crates/harness-core/src/context.rs') },
    ],
    more: { label: 'Security: secrets', href: doc('project/security-model', '#secrets') },
  },
  {
    id: 'log',
    step: 6,
    name: 'Event record',
    owner: 'Bridle',
    tone: 'bridle',
    line: 'writes every decision down',
    summary:
      'Each decision is one line in `.bridle/tool-events.jsonl`, with its class, verdict, rule and reason, and without secret values. With the task workflow on, a journal per task is the source of truth for `bridle trace` and `bridle explain`.',
    owns: ['The decision log, the task journal and the commands that read them.', 'Recovery state: checkpoints, run locks and leases under the Git layer.'],
    not: 'The log records what Bridle saw. It does not see what a command did once it was allowed to run, and a command that run_command refuses is recorded as that tool’s output, not as a separate decision.',
    fails: 'A damaged task cache is rebuilt from the journal with `bridle task repair`.',
    code: [
      { label: 'event_log.rs', href: links.source('crates/harness-runtime/src/event_log.rs') },
      { label: 'journal.rs', href: links.source('crates/harness-core/src/journal.rs') },
    ],
    more: { label: 'Tasks and loops', href: doc('concepts/tasks-and-loops') },
  },
];

/** The stages a call passes, in order, for each outcome. */
export const paths: Record<'allow' | 'deny', StageId[]> = {
  allow: ['model', 'host', 'policy', 'tool', 'result', 'log'],
  deny: ['model', 'host', 'policy', 'log'],
};

export const crates = [
  { name: 'harness-core', text: 'Configuration, the policy, the command classifier, redaction and loop rules. No network dependencies.', href: links.tree('crates/harness-core') },
  { name: 'harness-runtime', text: 'Model providers, the server launcher, the video-memory check, the event log, and the MCP server with its tools.', href: links.tree('crates/harness-runtime') },
  { name: 'harness-cli', text: 'The commands, the chat agent loop, the terminal UI and the OpenCode adapter.', href: links.tree('crates/harness-cli') },
  { name: 'bridle-desktop', text: 'The `bridle` program: the Tauri 2 window and the same commands for the terminal.', href: links.tree('crates/bridle-desktop') },
];
