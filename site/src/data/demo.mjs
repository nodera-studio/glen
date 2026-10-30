// @ts-check
// The hero terminal script. Every line of glen stdout comes from demo-capture.json: real glen
// output, captured by tests/site_demo.rs from the synthetic sessions in tests/fixtures/demo and
// decoded here (ansi.mjs). Time is compressed. The only hand-authored glen text is what glen
// writes to a TTY's stderr, which a capture cannot see: the send/wait spinner messages
// (src/commands/wait.rs job_progress) and the tail -f diagnostic (src/commands/tail.rs).
// terminal-player.test.mjs checks each of those strings against glen's source.
// Refresh the capture with `npm run demo:refresh`.
import capture from './demo-capture.json' with { type: 'json' };
import palette from '../../../src/render/palette.json' with { type: 'json' };
import { decode } from './ansi.mjs';

/** @typedef {'muted'|'border'|'accent'|'accent_2'|'success'|'warning'|'danger'|'focus'} Tone */
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
/** @param {Line} line */
export const plain = line => line.map(span => span.text).join('');

/** @type {Record<string, {args: string[], columns: number, stdout: string}>} */
const captures = capture.captures;
/** @type {Map<string, Line[]>} */
const decoded = new Map();
/** The decoded capture for `key` (`<command>@<columns>`). @param {string} key @returns {Line[]} */
export function lines(key) {
 const raw = captures[key];
 if (!raw) throw new Error(`no demo capture ${key}; add it to tests/site_demo.rs`);
 if (!decoded.has(key)) decoded.set(key, decode(raw.stdout, palette));
 return /** @type {Line[]} */ (decoded.get(key));
}

/** Split lines on the empty lines between them (render_items joins items with a blank line). @param {Line[]} list */
const blocks = list => list.reduce((out, line) => { if (plain(line) === '') out.push([]); else out[out.length - 1].push(line); return out; }, /** @type {Line[][]} */ ([[]]));

/** `glen ls`. @param {number} w */
export const lsTable = w => lines('ls@' + w);
/** The `glen wait` outcome; `glen send` prints the same when the reply arrives. @param {number} w */
export const completedBlock = w => lines('wait@' + w);
/** `glen show` header: title and meta, before the first empty line. @param {number} w */
export const showHeader = w => blocks(lines('show@' + w))[0];
/** `glen show` transcript items, oldest first. @param {number} w */
export const items = w => blocks(lines('show@' + w)).slice(1);
/** `glen tail -f` stdout before the first new item: its short header. @param {number} w */
export const tailHeader = w => lines('tail-f@' + w);
/**
 * An item split so each part starts with the head (role line plus thinking or text) or with a
 * tool row (`  ↳ `), wrapped continuation lines included.
 * @param {Line[]} item
 */
export const itemParts = item => item.reduce((out, line, i) => { if (i && plain(line).startsWith('  ↳ ')) out.push([line]); else out[out.length - 1].push(line); return out; }, /** @type {Line[][]} */ ([[]]));

const outcome = plain(completedBlock(76)[0]).match(/^✓ completed · (j[0-9a-f]+) · ([0-9a-f]+) · /);
if (!outcome) throw new Error('wait@76 does not start with a completed outcome');
/** The job handle and session handle glen printed. */
export const [, job, session] = outcome;
/** Index of the message the agent sends: the last user item in the transcript. */
const sendAt = items(76).findLastIndex(item => plain(item[0]).startsWith('❯ user'));
/** The message the agent sends, as the user item's body shows it. */
export const message = items(76)[sendAt].slice(1).map(l => plain(l).trim()).join(' ');
/** Text of the reply line the selection cue highlights: glen's `↩ <reply>` line without its tick. */
export const selection = plain(/** @type {Line} */ (completedBlock(76).find(l => plain(l).startsWith('↩ ')))).replace(/ ✓$/, '');

/**
 * wait.rs job_progress() on the live route: the spinner's format string for each job state (TTY
 * stderr, not captured). The demo session is live, so glen send and glen wait both show these.
 */
export const LIVE_PROGRESS = { queued: 'delivering to {} (live) · {}', accepted: 'delivered as {}; omp is working' };
/** How the glen extension takes a message for an idle TUI session (extension/glen/index.ts). */
export const deliveredAs = 'prompt';
/** Fill a Rust format string's `{}` placeholders in order. @param {string} f @param {...string} args */
const format = (f, ...args) => args.reduce((out, a) => out.replace('{}', () => a), f);
/** The demo job's spinner messages, in job order: queued while glen delivers, then accepted. */
export const PROGRESS = { queued: format(LIVE_PROGRESS.queued, session, job), accepted: format(LIVE_PROGRESS.accepted, deliveredAs) };

export const prompt = [s(cwd, 'accent_2'), s(' '), s('❯', 'accent'), s(' ')];
const sendCmd = `glen send ${session} "${message}"`;
/** One transcript item as the omp pane prints it: a blank line, then the item. @param {Line[]} item */
const item = item => [[], ...item];
/** The transcript after the send, split into the steps the omp pane shows them in. @param {number} w */
const turn = w => {
 const [user, work, reply, answer] = items(w).slice(sendAt);
 const [head, edit, bash] = itemParts(work);
 return { user, head, edit, bash, reply, answer };
};

/** @type {Step[]} */
export const script = [
 { k: 'static', p: 'b', lines: w => [
  [...prompt, s(`glen tail -f ${session}`)],
  ...tailHeader(w),
  [s(`following ${session} · Ctrl-C to stop`, 'muted')],
  ...items(w).slice(0, sendAt).flatMap(item),
 ] },
 { k: 'cmd', p: 'a', id: 'ls', think: 420, text: 'glen ls' },
 { k: 'title', after: 'ls', v: 'glen' },
 { k: 'flush', p: 'a', after: 'ls', dt: 140, lines: lsTable },
 { k: 'title', dt: 90, v: 'zsh' },
 { k: 'cmd', p: 'a', id: 'send', dt: 0, think: 350, text: sendCmd },
 { k: 'title', after: 'send', v: 'glen' },
 // The spinner starts on the queued job while glen delivers through the session's glen
 // extension, then switches once the extension accepts it, just before omp shows the message.
 { k: 'spin', p: 'a', id: 'spin', after: 'send', dt: 30, v: PROGRESS.queued },
 { k: 'event', p: 'b', after: 'send', dt: 500, lines: w => item(turn(w).user) },
 { k: 'msg', after: 'send', dt: 480, v: PROGRESS.accepted },
 { k: 'focus', p: 'b', after: 'send', dt: 540 },
 { k: 'event', p: 'b', after: 'send', dt: 1400, lines: w => item(turn(w).head) },
 { k: 'event', p: 'b', after: 'send', dt: 2300, lines: w => turn(w).edit },
 { k: 'event', p: 'b', after: 'send', dt: 4200, lines: w => turn(w).bash },
 // glen_reply is its own assistant message; its tool row lands at 5500, just before the hop.
 { k: 'event', p: 'b', id: 'reply', after: 'send', dt: 5420, lines: w => item(turn(w).reply) },
 { k: 'hop', after: 'send', dt: 5560 },
 { k: 'focus', p: 'a', after: 'send', dt: 6080 },
 { k: 'flush', p: 'a', after: 'send', dt: 6100, ends: 'spin', lines: completedBlock },
 { k: 'title', dt: 60, v: 'zsh' },
 { k: 'prompt', p: 'a', dt: 0 },
 { k: 'event', p: 'b', after: 'send', dt: 6240, lines: w => item(turn(w).answer) },
 { k: 'sel', p: 'a', after: 'send', dt: 7100 },
 { k: 'done', after: 'send', dt: 9300 },
];
