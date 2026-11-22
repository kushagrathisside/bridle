// The policy lab: how a recorded answer is laid out as the six stages of a call. The final class, verdict, rule and message are the
// recorded output of the real policy. The order of the checks follows `evaluate` in crates/harness-core/src/security/tool_policy.rs;
// a check that comes before the rule that fired is shown as passed, and one that comes after as not reached.
import { cases, loop, reasonOf, stamp, type HookAnswer, type RecordedCase, type Tone } from './policy';

export type CheckState = 'pass' | 'fail' | 'skip';

export interface Check {
  rule: string;
  title: string;
  question: string;
  state: CheckState;
  detail: string;
}

interface CheckSpec {
  rule: string;
  title: string;
  question: string;
}

const SPECS: Record<string, CheckSpec> = {
  secret_env: { rule: 'secret_env', title: 'Environment', question: 'Does the command print the environment, where this machine’s secrets live?' },
  secret_file: { rule: 'secret_file', title: 'Secret files', question: 'Does any path or shell argument name credentials, such as .env, a key or a certificate?' },
  path_guard: { rule: 'path_guard', title: 'Paths', question: 'Does every path stay inside the project, with no device names, streams or UNC paths?' },
  protected_path: { rule: 'protected_path', title: 'Control files', question: 'Does it write bridle.toml, .bridle, .opencode, .worktrees or .git?' },
  command_risk: { rule: 'command_risk', title: 'Command class', question: 'Classify the command by program and arguments, then look up its verdict.' },
  tool_kind: { rule: 'tool_kind', title: 'Tool family', question: 'Classify the tool, then look up its verdict.' },
  unresolved_path: { rule: 'unresolved_path', title: 'Run-time arguments', question: 'Does an argument depend on something only the shell can resolve, such as $HOME or $(...)?' },
  loop_rule: { rule: 'loop_rule', title: 'Loop rules', question: 'Has this call been repeated since the last verification? (Task workflow on.)' },
};

/** The checks a tool family runs, in the order the policy runs them. */
function sequence(tool: string): string[] {
  if (tool === 'bash') return ['secret_env', 'secret_file', 'path_guard', 'command_risk', 'protected_path', 'unresolved_path'];
  if (tool === 'edit' || tool === 'write') return ['secret_file', 'path_guard', 'protected_path', 'tool_kind'];
  if (tool === 'webfetch') return ['tool_kind'];
  return ['secret_file', 'path_guard', 'tool_kind'];
}

const FAMILY: Record<string, string> = {
  read: 'Read',
  edit: 'Edit',
  write: 'Write',
  bash: 'Shell',
  webfetch: 'Web fetch',
  glob: 'Search',
  grep: 'Search',
};

export function family(tool: string): string {
  return FAMILY[tool] ?? tool;
}

const PASS: Record<string, string> = {
  secret_env: 'It does not print the environment.',
  secret_file: 'No path or argument names a secret file.',
  path_guard: 'Every path resolves inside the project.',
  protected_path: 'It does not touch a control file.',
  command_risk: 'Classified; see the decision.',
  tool_kind: 'Classified; see the decision.',
  unresolved_path: 'Every argument can be resolved before it runs.',
  loop_rule: 'No loop rule fired.',
};

export function checksFor(tool: string, answer: HookAnswer, repeated = false): Check[] {
  const order = [...sequence(tool), ...(repeated ? ['loop_rule'] : [])];
  // `allow_pattern_bypass` is decided inside the command classifier, so the classifier is the check that fired.
  const fired = answer.rule === 'allow_pattern_bypass' ? 'command_risk' : answer.rule;
  const at = answer.verdict === 'allow' && answer.rule !== 'loop_rule' ? -1 : order.indexOf(fired ?? '');
  return order.map((rule, index) => {
    const spec = SPECS[rule];
    // The rule that gives an allow is the last stage: it passes with the class it found.
    const last = index === order.length - 1 && answer.verdict === 'allow';
    const state: CheckState = at === -1 ? 'pass' : index < at ? 'pass' : index === at ? 'fail' : 'skip';
    let detail = PASS[rule];
    if (state === 'fail') detail = reasonOf(answer);
    else if (state === 'skip') detail = 'Not reached: an earlier check already decided.';
    else if ((last || rule === answer.rule) && rule !== 'loop_rule') detail = `${answer.class ?? ''} → ${answer.verdict ?? 'allow'}`.trim();
    return { rule, title: spec.title, question: spec.question, state, detail };
  });
}

export interface Outcome {
  tone: Tone;
  label: string;
  runs: boolean;
  /** One sentence for the "execution or rejection" stage. */
  happens: string;
  /** One sentence for the last stage: what the model is told, or what happens to the output. */
  result: string;
}

export function outcomeOf(item: RecordedCase['answer'], tool: string): Outcome {
  const mark = stamp(item);
  const runs = item.action === 'allow';
  const writes = tool === 'edit' || tool === 'write';
  return {
    tone: mark.tone,
    label: mark.text,
    runs,
    happens: runs
      ? writes
        ? 'The edit is applied, atomically, inside the project.'
        : tool === 'bash'
          ? 'The command runs under your user account. Bridle is not a sandbox.'
          : 'The tool runs.'
      : 'The call does not run. Nothing was changed.',
    result: runs
      ? 'The output is bounded and redacted before the model reads it, and the decision is written to the event log.'
      : `The model receives the reason, “${reasonOf(item)}”, and the decision is written to the event log.`,
  };
}

/** The JSON the OpenCode plugin sends to `bridle hook tool-before`. */
export function payloadOf(item: RecordedCase): string {
  return JSON.stringify({ sessionID: 's1', callID: 'c1', tool: item.tool, args: item.args });
}

export const GROUPS: { id: RecordedCase['group'] | 'repeat'; label: string }[] = [
  { id: 'files', label: 'Files' },
  { id: 'secrets', label: 'Secrets' },
  { id: 'shell', label: 'Shell' },
  { id: 'network', label: 'Network' },
  { id: 'repeat', label: 'Repeated' },
];

export const groups = GROUPS.map((group) => ({ ...group, items: cases.filter((item) => item.group === group.id) }));
export { loop };
