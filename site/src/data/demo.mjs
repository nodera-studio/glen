// @ts-check
// The hero terminal script. Sessions are synthetic and time is compressed, but every line is
// laid out the way glen prints it: ls table and column dropping (src/commands/ls.rs:181-235,
// src/render/table.rs), transcript roles, tool rows and wrapping (src/render/transcript.rs,
// src/render/text.rs:37), the tail -f short header and diagnostic (src/commands/tail.rs:254-268),
// the send spinner (W2 send.rs:166-191) and the wait outcome (W2 wait.rs:96-120).
// W7/W8: regenerate from real `glen` output against tests/fixtures (D8).

/** @typedef {'muted'|'accent'|'accent_2'|'success'|'warning'|'danger'|'focus'} Tone */
/** @typedef {{text: string, tone?: Tone, bold?: boolean}} Span */
/** @typedef {Span[]} Line */
/** @typedef {'a'|'b'} Pane */
/**
 * One script step. `after` anchors `dt` to a named step's end (a command's Enter); otherwise
 * `dt` counts from the previous step's end.
 * @typedef {{
 *  k: 'static'|'cmd'|'flush'|'event'|'spin'|'title'|'msg'|'focus'|'hop'|'sel'|'prompt'|'done',
 *  p?: Pane, id?: string, after?: string, dt?: number, think?: number, text?: string,
 *  v?: string, ends?: string, lines?: (cols: number) => Line[]
 * }} Step
 */

export const cwd = '~/work/alpha';
export const SEED = 7;
/** Terminal widths in columns. Wide: two panes side by side; narrow: the 44-column layout. */
export const COLS = { wide: { a: 76, b: 64 }, narrow: { a: 44, b: 44 } };
/** Visible rows per pane. */
export const ROWS = { wide: 17, narrow: 15 };
/** Typing windows (first key to Enter), the table the unit tests hold the planner to. */
export const WINDOWS = { ls: [420, 1000], send: [1600, 4700] };

/** @param {string} text @param {Tone} [tone] @param {boolean} [bold] @returns {Span} */
export const s = (text, tone, bold) => ({ text, ...(tone ? { tone } : {}), ...(bold ? { bold } : {}) });
/** Display width; every glyph glen prints here is single-width. @param {string} t */
const width = t => [...t].length;
/** text::truncate @param {string} t @param {number} n */
const truncate = (t, n) => width(t) <= n ? t : [...t].slice(0, Math.max(0, n - 1)).join('') + '…';
/** @param {Line} line */
export const plain = line => line.map(span => span.text).join('');

/** text::wrap: word wrap that keeps the leading indent. @param {string} line @param {number} max */
export function wrap(line, max) {
 if (width(line) <= max) return [line];
 const indent = truncate(line.match(/^\s*/)?.[0] ?? '', max - 1);
 const out = [];
 let current = indent;
 for (const word of line.split(/\s+/).filter(Boolean)) {
  if (width(current) > width(indent) && width(current) + 1 + width(word) <= max) { current += ' ' + word; continue; }
  if (width(current) > width(indent)) { out.push(current); current = indent; }
  for (const c of word) {
   if (width(current) + 1 > max) { out.push(current); current = indent; }
   current += c;
  }
 }
 if (width(current) > width(indent)) out.push(current);
 return out;
}

export const prompt = [s(cwd, 'accent_2'), s(' '), s('❯', 'accent'), s(' ')];

const sessions = [
 { h: 'e27c0de', live: true, title: 'Fix migration 0012', turn: 'complete', updated: '2s ago', model: 'claude-sonnet-4-5' },
 { h: 'b7e2d81', live: false, title: 'Review routes', turn: 'complete', updated: '3m ago', model: 'claude-sonnet-4-5' },
 { h: 'b8e4a07', live: false, title: 'Add fixtures', turn: 'complete', updated: '1h ago', model: 'claude-sonnet-4-5' },
];

/** `glen ls` for one project at a terminal width (ls.rs column_budget + table.rs). @param {number} w @returns {Line[]} */
export function lsTable(w) {
 const labels = ['handle', 'state', 'project', 'title', 'turn', 'updated', 'model'];
 const cells = sessions.map(r => [r.h, r.live ? '● live' : '○ idle', cwd, r.title, r.turn, r.updated, r.model]);
 const widths = labels.map((label, i) => Math.max(label.length, ...cells.map(c => width(c[i]))));
 let cols = [0, 1, 3, 4, 5, 6];
 const other = () => cols.filter(i => i !== 3).reduce((n, i) => n + widths[i], 0) + 2 * (cols.length - 1);
 for (const drop of [6, 4, 2]) { if (w - other() >= 16) break; cols = cols.filter(i => i !== drop); }
 const budgets = widths.slice();
 budgets[3] = Math.max(1, w - other());
 const head = cols.map(i => truncate(labels[i], budgets[i]));
 const body = cells.map(c => cols.map(i => truncate(c[i], budgets[i])));
 const colWidth = cols.map((_, j) => Math.max(width(head[j]), ...body.map(r => width(r[j]))));
 /** @param {(text: string, j: number) => Span[]} paint @param {string[]} row @returns {Line} */
 const line = (row, paint) => {
  const spans = row.flatMap((text, j) => {
   const last = j === row.length - 1;
   const pad = last ? '' : ' '.repeat(colWidth[j] - width(text) + 2);
   return [...paint(text, j), ...(pad ? [s(pad)] : [])];
  });
  return spans.filter(span => span.text);
 };
 const lines = [line(head, text => [s(text, 'muted', true)])];
 body.forEach((row, r) => lines.push(line(row, (text, j) => {
  const i = cols[j];
  if (i === 0) return [s(text, 'focus')];
  if (i === 1) return sessions[r].live ? [s('●', 'success'), s(' '), s('live', 'success')] : [s('○', 'muted'), s(' '), s('idle', 'muted')];
  if (i >= 4) return [s(text, 'muted')];
  return [s(text)];
 })));
 lines.push([], [s(`${sessions.length} sessions · ${sessions.filter(r => r.live).length} live`, 'muted')]);
 return lines;
}

/** transcript.rs body(): wrap at width - 2 behind a two-space indent. @param {string} text @param {number} w @param {Tone} [tone] @returns {Line[]} */
export const body = (text, w, tone) => wrap(text, w - 2).map(l => [s('  '), s(l, tone)]);
/** transcript.rs role() @param {'user'|'assistant'} who @param {string} meta @returns {Line} */
export const role = (who, meta) => who === 'user' ? [s('❯ user', 'accent', true), s(` · ${meta}`, 'muted')] : [s('◆ assistant', 'accent_2', true), s(` · ${meta}`, 'muted')];
/** transcript.rs tool(): collapsed row, or plain wrapped body when it does not fit. @returns {(w: number) => Line[]} */
export const tool = (/** @type {string} */ name, /** @type {string} */ summary, /** @type {string} */ result) => w => {
 const row = [s('  ↳ '), s(name, undefined, true), s(' '), s(summary, 'muted'), s(' · '), s('✓', 'success'), s(` ${result}`)];
 return width(plain(row)) <= w ? [row] : wrap(`↳ ${name} ${summary} · ✓ ${result}`, w - 2).map(l => [s('  ' + l)]);
};

export const reply = 'Added the index. All 12 tests pass.';
export const answer = `${reply} Migration 0012 is ready.`;
const sendCmd = 'glen send e27c0de "Fix 0012 and tell me when it passes."';

/** @type {Step[]} */
export const script = [
 { k: 'static', p: 'b', lines: w => [
  [...prompt, s('glen tail -f e27c0de')],
  [s('e27c0de · Fix migration 0012 · '), s('●', 'success'), s(' '), s('live', 'success')],
  [s('following e27c0de · Ctrl-C to stop', 'muted')],
  [],
  role('assistant', 'claude-sonnet-4-5 · 12:01'),
  ...body('Found a missing index on users.email.', w),
 ] },
 { k: 'cmd', p: 'a', id: 'ls', think: 420, text: 'glen ls' },
 { k: 'title', after: 'ls', v: 'glen' },
 { k: 'flush', p: 'a', after: 'ls', dt: 140, lines: lsTable },
 { k: 'title', dt: 90, v: 'zsh' },
 { k: 'cmd', p: 'a', id: 'send', dt: 0, think: 350, text: sendCmd },
 { k: 'title', after: 'send', v: 'glen' },
 { k: 'spin', p: 'a', id: 'spin', after: 'send', dt: 30, v: 'sending to e27c0de · j3c9f41' },
 { k: 'event', p: 'b', after: 'send', dt: 500, lines: w => [[], role('user', '12:04'), ...body('Fix 0012 and tell me when it passes.', w)] },
 { k: 'focus', p: 'b', after: 'send', dt: 540 },
 { k: 'msg', after: 'send', dt: 820, v: 'omp is working' },
 { k: 'event', p: 'b', after: 'send', dt: 1400, lines: w => [[], role('assistant', '12:04'), ...body('∴ thinking · 18 words', w, 'muted')] },
 { k: 'event', p: 'b', after: 'send', dt: 2300, lines: tool('edit', 'migrations/0012_add_index.sql', '3 lines') },
 { k: 'event', p: 'b', after: 'send', dt: 4200, lines: tool('bash', 'cargo test migration_0012', '14 lines') },
 { k: 'event', p: 'b', id: 'reply', after: 'send', dt: 5500, lines: tool('glen_reply', reply, 'delivered') },
 { k: 'hop', after: 'send', dt: 5560 },
 { k: 'focus', p: 'a', after: 'send', dt: 6080 },
 { k: 'flush', p: 'a', after: 'send', dt: 6100, ends: 'spin', lines: w => [
  [s('✓ completed · j3c9f41 · e27c0de · 41207ms', 'success')],
  [],
  ...wrap(`  ${answer}`, w).map(l => [s(l)]),
  [s(`↩ ${reply} ✓`, 'muted')],
 ] },
 { k: 'title', dt: 60, v: 'zsh' },
 { k: 'prompt', p: 'a', dt: 0 },
 { k: 'event', p: 'b', after: 'send', dt: 6240, lines: w => [[], role('assistant', '12:04'), ...body(answer, w)] },
 { k: 'sel', p: 'a', after: 'send', dt: 7100 },
 { k: 'done', after: 'send', dt: 9300 },
];

/** Text of the reply line the selection cue highlights. */
export const selection = `↩ ${reply}`;
