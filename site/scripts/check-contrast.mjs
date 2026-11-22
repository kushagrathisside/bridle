// Checks the WCAG contrast of the text colours in src/styles/tokens.css against the surfaces they sit on, for both themes.
// Run: node scripts/check-contrast.mjs   (exit code 1 when a pair is below its minimum)
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const css = fs.readFileSync(path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..', 'src', 'styles', 'tokens.css'), 'utf8');

// The dark theme is :root, the light theme is :root[data-theme='light'].
const block = (start) => {
  const from = css.indexOf(start);
  return css.slice(from, css.indexOf('}', from));
};
const read = (text) => Object.fromEntries([...text.matchAll(/--([\w-]+):\s*(#[0-9a-f]{6})/gi)].map((m) => [m[1], m[2]]));
const dark = read(block(':root {'));
const light = { ...dark, ...read(block(":root[data-theme='light']")) };

const lum = (hex) => {
  const channel = (i) => {
    const v = parseInt(hex.slice(1 + i * 2, 3 + i * 2), 16) / 255;
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(0) + 0.7152 * channel(1) + 0.0722 * channel(2);
};
const ratio = (a, b) => {
  const [hi, lo] = [lum(a), lum(b)].sort((x, y) => y - x);
  return (hi + 0.05) / (lo + 0.05);
};

// [foreground token, background token, minimum]
const PAIRS = [
  ['text', 'bg', 4.5], ['text-2', 'bg', 4.5], ['text-3', 'bg', 4.5],
  ['text', 'bg-1', 4.5], ['text-2', 'bg-1', 4.5], ['text-3', 'bg-1', 4.5],
  ['text', 'bg-2', 4.5], ['text-2', 'bg-2', 4.5], ['text-3', 'bg-2', 4.5],
  ['text', 'bg-code', 4.5], ['text-2', 'bg-code', 4.5],
  ['accent', 'bg', 4.5], ['accent', 'bg-1', 4.5], ['accent', 'accent-soft', 4.5], ['accent-ink', 'accent', 4.5],
  ['allow', 'bg', 4.5], ['allow', 'bg-1', 4.5], ['allow', 'allow-soft', 4.5],
  ['ask', 'bg', 4.5], ['ask', 'bg-1', 4.5], ['ask', 'ask-soft', 4.5],
  ['deny', 'bg', 4.5], ['deny', 'bg-1', 4.5], ['deny', 'deny-soft', 4.5],
  ['line-2', 'bg', 1.4],
];

let failures = 0;
for (const [name, theme] of [['dark', dark], ['light', light]]) {
  for (const [fg, bg, min] of PAIRS) {
    const value = ratio(theme[fg], theme[bg]);
    const ok = value >= min;
    if (!ok) failures += 1;
    if (!ok || process.argv.includes('--all')) console.log(`${ok ? 'ok  ' : 'FAIL'} ${name.padEnd(5)} ${fg} on ${bg}: ${value.toFixed(2)} (needs ${min})`);
  }
}
console.log(failures === 0 ? `contrast: all ${PAIRS.length * 2} pairs pass` : `contrast: ${failures} pair(s) below the minimum`);
process.exit(failures === 0 ? 0 : 1);
