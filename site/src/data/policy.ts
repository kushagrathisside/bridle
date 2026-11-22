// The recorded answers of the real policy (see scripts/record-policy.mjs), and the small helpers the page uses to show them.
import recorded from './policy-recorded.json';

export type Verdict = 'allow' | 'ask' | 'deny';
export type Tone = 'allow' | 'ask' | 'deny';

export interface HookAnswer {
  action: 'allow' | 'block';
  message: string;
  class?: string;
  verdict?: Verdict;
  rule?: string;
}

export interface RecordedCase {
  id: string;
  group: 'files' | 'secrets' | 'shell' | 'network';
  label: string;
  tool: string;
  args: Record<string, string>;
  shows: string;
  limit?: boolean;
  answer: HookAnswer;
}

export const meta = {
  bridle: recorded.bridle as string,
  system: recorded.system as string,
  profile: recorded.profile as string,
  recordedAt: recorded.recordedAt as string,
};

export const cases = recorded.cases as unknown as RecordedCase[];
export const loop = recorded.loop as unknown as { n: number; answer: HookAnswer }[];
export const failClosed = recorded.failClosed as unknown as HookAnswer;
export const redaction = recorded.redaction as { before: string; after: string }[];
export const explain = recorded.explain as string;
export const eventLogSample = (recorded.eventLogSample as string[])[0] ?? '';

export function byId(id: string): RecordedCase {
  const found = cases.find((item) => item.id === id);
  if (!found) throw new Error(`site: no recorded policy case "${id}"; run scripts/record-policy.mjs`);
  return found;
}

/** The label and colour a verdict is shown with. `ask` is blocked, and the page says so. */
export function stamp(answer: HookAnswer): { text: string; tone: Tone } {
  if (answer.rule === 'loop_rule') return { text: 'Refused', tone: 'deny' };
  if (answer.verdict === 'allow') return { text: 'Allow', tone: 'allow' };
  if (answer.verdict === 'ask') return { text: 'Ask, so blocked', tone: 'ask' };
  return { text: 'Deny', tone: 'deny' };
}

/** The reason the policy gave, without the "blocked by policy (CLASS, rule):" prefix the hook adds. */
export function reasonOf(answer: HookAnswer): string {
  return answer.message.replace(/^blocked by policy \([^)]*\):\s*/, '').replace(/^refused by loop rule \w+:\s*/, '');
}
