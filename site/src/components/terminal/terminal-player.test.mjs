import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { plan, total, paneAt, typeDelays, mulberry32, SPINNER_TICKS, TICK_MS, ROW_KINDS, CUE_KINDS, TYPE } from './terminal-player.mjs';
import { script, COLS, ROWS, WINDOWS, SEED, lsTable, completedBlock, items, tailHeader, lines, plain, selection, job, session, message, PROGRESS } from '../../data/demo.mjs';
import capture from '../../data/demo-capture.json' with { type: 'json' };

const wide = plan(script, { seed: SEED });
const narrow = plan(script, { seed: SEED, narrow: true });
const narrowCols = plan(script, { seed: SEED, cols: COLS.narrow });
const sample = cues => { const end = total(cues); const times = []; for (let t = 0; t <= end; t += 10) times.push(t); return times.concat(cues.map(c => c.at), cues.flatMap(c => [c.end, c.commit]).filter(x => x !== undefined)); };

test('plan is deterministic for seed 7 and ends within 15 s', () => {
 assert.deepEqual(plan(script, { seed: 7 }), wide);
 assert.notDeepEqual(plan(script, { seed: 8 }).filter(c => c.ty).map(c => c.ty), wide.filter(c => c.ty).map(c => c.ty));
 assert.ok(total(wide) <= 15000, `total ${total(wide)}`);
 assert.equal(total(narrow), total(wide));
});

test('each typed command takes its table window, within 25%', () => {
 const commands = wide.filter(c => c.k === 'cmd' && c.line);
 assert.equal(commands.length, 2);
 for (const [cue, [start, end]] of [[commands[0], WINDOWS.ls], [commands[1], WINDOWS.send]]) {
  const took = cue.commit - cue.ty[0], window = end - start;
  assert.ok(Math.abs(took - window) <= window * .25, `${cue.text}: ${took} ms vs ${window} ms`);
  assert.ok(Math.abs(cue.ty[0] - start) <= window * .25, `${cue.text} starts at ${cue.ty[0]}`);
 }
});

test('typing model: first key at once, jitter in range, pauses after spaces and quotes, a glance on long commands', () => {
 const text = 'glen send e27c0de "Fix 0012 and tell me when it passes."';
 const d = typeDelays(text, mulberry32(7));
 assert.equal(d[0], 0);
 assert.equal(d.length, text.length);
 for (let i = 1; i < d.length; i++) {
  let lo = TYPE.base + TYPE.lo, hi = TYPE.base + TYPE.hi;
  if (text[i - 1] === ' ') { lo += TYPE.space; hi += TYPE.space; } else if ('-/"'.includes(text[i - 1])) { lo += TYPE.punct; hi += TYPE.punct; }
  if (i === TYPE.glanceAt) { lo += TYPE.glance; hi += TYPE.glance; }
  assert.ok(d[i] >= Math.floor(lo) && d[i] <= Math.ceil(hi), `char ${i}: ${d[i]} not in ${lo}..${hi}`);
 }
 assert.equal(typeDelays('glen ls', mulberry32(7)).reduce((a, b) => a + b) < 7 * (TYPE.base + TYPE.hi) + TYPE.space, true, 'short commands get no glance');
});

test('SPINNER_TICKS is indicatif’s default set, one frame every 80 ms', () => {
 // indicatif 0.18 src/style.rs: tick_strings "⠁⠁⠉⠙⠚⠒⠂⠂⠒⠲⠴⠤⠄⠄⠤⠠⠠⠤⠦⠖⠒⠐⠐⠒⠓⠋⠉⠈⠈ " minus the finished blank.
 assert.equal(SPINNER_TICKS, '⠁⠁⠉⠙⠚⠒⠂⠂⠒⠲⠴⠤⠄⠄⠤⠠⠠⠤⠦⠖⠒⠐⠐⠒⠓⠋⠉⠈⠈');
 assert.equal([...SPINNER_TICKS].length, 29);
 assert.equal(TICK_MS, 80);
});

test('the scroll offset never hides the newest row or the cursor line', () => {
 for (const [cues, rows] of [[wide, ROWS.wide], [narrowCols, ROWS.narrow]]) for (const pane of ['a', 'b']) for (const t of sample(cues)) {
  const { on, offset } = paneAt(cues, pane, t, rows);
  const newest = on.length - 1 - offset;
  assert.ok(offset >= 0);
  // Upper bounds: neither the newest row nor the cursor line below it falls under the pane.
  assert.ok(newest < rows, `${pane}@${t}: newest row ${on.length - 1} below the pane (offset ${offset})`);
  assert.ok(on.length - offset <= rows, `${pane}@${t}: cursor line below the pane`);
  // Lower bounds: the newest row never scrolls above the pane, and the pane scrolls no further
  // than the newest row and its cursor line need, so it stays full.
  if (on.length) assert.ok(newest >= 0, `${pane}@${t}: newest row ${on.length - 1} above the pane (offset ${offset})`);
  assert.ok(on.length - offset >= Math.min(on.length, rows - 1), `${pane}@${t}: over-scrolled, ${on.length - offset} of ${on.length} rows showing`);
 }
 // The omp pane outgrows the wide window by the end, so the final frame scrolls.
 assert.ok(paneAt(wide, 'b', total(wide), ROWS.wide).offset > 0);
});

test('rows become visible in order, and the spinner is replaced by the completion line', () => {
 for (const pane of ['a', 'b']) {
  const rows = wide.filter(c => c.p === pane && ROW_KINDS.includes(c.k));
  rows.forEach((row, i) => assert.ok(!i || row.at >= rows[i - 1].at, `${pane} row ${i} goes back in time`));
 }
 const spin = wide.find(c => c.k === 'spin');
 const done = wide.find(c => c.k === 'flush' && c.at === spin.end);
 assert.ok(done && plain(done.line).startsWith(`✓ completed · ${job} · ${session} · `));
 const send = wide.find(c => c.k === 'cmd' && c.text.startsWith('glen send'));
 assert.ok(spin.at >= send.commit, 'spinner starts after Enter');
 const reply = wide.find(c => c.k === 'event' && plain(c.line).startsWith('  ↳ glen_reply'));
 const hop = wide.find(c => c.k === 'hop');
 assert.ok(hop.at >= reply.at && hop.at < spin.end, 'the reply hops across before glen prints it');
});

test('glen send and wait output is W2’s: spinner messages in job order, then the captured outcome', () => {
 // W2 job handles are j plus 7 hex digits (src/jobs/mod.rs JobId::handle).
 assert.match(job, /^j[0-9a-f]{7}$/);
 const spin = wide.find(c => c.k === 'spin');
 assert.equal(spin.v, `sending to ${session} · ${job}`);
 const msgs = wide.filter(c => c.k === 'msg');
 assert.deepEqual(msgs.map(c => c.v), ['starting omp', 'waiting for the reply']);
 for (const m of msgs) assert.ok(m.at > spin.at && m.at < spin.end, `${m.v} shows while the spinner does`);
 const user = wide.find(c => c.k === 'event' && plain(c.line).startsWith('❯ user'));
 assert.ok(msgs[1].at <= user.at, 'omp accepts the prompt before its user message shows');
 for (const cues of [wide, narrowCols]) {
  const done = cues.find(c => c.k === 'flush' && c.at === cues.find(x => x.k === 'spin').end);
  assert.deepEqual(done.line, [{ text: `✓ completed · ${job} · ${session} · 41s`, tone: 'success' }]);
 }
});

test('the tail -f diagnostic is the one glen’s source prints to a TTY', () => {
 const tail = readFileSync(fileURLToPath(new URL('../../../../src/commands/tail.rs', import.meta.url)), 'utf8');
 assert.ok(tail.includes('"following {} · Ctrl-C to stop"'));
 const diag = wide.filter(c => c.p === 'b' && c.line && plain(c.line) === `following ${session} · Ctrl-C to stop`);
 assert.equal(diag.length, 1);
 assert.deepEqual(diag[0].line, [{ text: `following ${session} · Ctrl-C to stop`, tone: 'muted' }]);
});

const w2 = fileURLToPath(new URL('../../../../src/commands/wait.rs', import.meta.url));
test('the demo strings are the ones glen’s source prints', { skip: !existsSync(w2) && 'W2 (src/commands/wait.rs) is not on this branch yet' }, () => {
 const wait = readFileSync(w2, 'utf8'), send = readFileSync(w2.replace(/wait\.rs$/, 'send.rs'), 'utf8');
 for (const v of Object.values(PROGRESS)) assert.ok(wait.includes(`"${v}"`), v);
 for (const f of ['format!("{seconds}s")', 'format!("{}m {:02}s"', '"✓ completed · {} · {}"']) assert.ok(wait.includes(f), f);
 assert.ok(send.includes('"sending to {} · {}"'));
});

test('narrow mode turns the hop into a tab switch', () => {
 assert.equal(narrow.some(c => c.k === 'hop'), false);
 const hop = wide.find(c => c.k === 'hop');
 assert.ok(narrow.some(c => c.k === 'focus' && c.p === 'a' && c.at === hop.at));
});

test('every cue kind is known, and every line fits its pane', () => {
 for (const [cues, cols] of [[wide, COLS.wide], [narrowCols, COLS.narrow]]) for (const c of cues) {
  assert.ok(ROW_KINDS.includes(c.k) || CUE_KINDS.includes(c.k) || c.k === 'focus', c.k);
  if (c.line) assert.ok([...plain(c.line)].length + (c.k === 'cmd' ? 15 + c.text.length : 0) <= cols[c.p], `${c.p}: ${plain(c.line)}`);
  if (c.k === 'cmd' && !c.line) assert.ok(c.text.length <= cols[c.p]);
 }
 const dist = fileURLToPath(new URL('../../../dist/index.html', import.meta.url));
 if (existsSync(dist)) {
  const kinds = new Set([...readFileSync(dist, 'utf8').matchAll(/data-k="([^"]+)"/g)].map(m => m[1]));
  assert.ok(kinds.size > 0);
  for (const k of kinds) assert.ok(ROW_KINDS.includes(k), `unknown data-k ${k}`);
 }
});

test('the narrow command wraps at the column like a shell, without losing a character', () => {
 const rows = narrowCols.filter(c => c.k === 'cmd' && c.text && c.ty.length && c.commit === narrowCols.find(x => x.text?.startsWith('glen send'))?.commit);
 assert.equal(rows.length, 2);
 assert.equal(rows.map(r => r.text).join(''), 'glen send e27c0de "Fix 0012 and tell me when it passes."');
 assert.equal(rows[0].text.length, COLS.narrow.a - 15);
 assert.equal(rows[1].at, rows[1].ty[0], 'the second line appears with its first key');
});

// AC-732: every line of glen stdout in the demo is the decoded capture, span for span.
for (const [name, cols] of Object.entries(COLS)) {
 const cues = plan(script, { seed: SEED, cols });
 test(`${name}: the agent pane prints the captured ls table and wait outcome`, () => {
  const flush = cues.filter(c => c.p === 'a' && c.k === 'flush').map(c => c.line);
  const ls = lsTable(cols.a);
  assert.deepEqual(flush.slice(0, ls.length), ls);
  assert.deepEqual(flush.slice(ls.length), completedBlock(cols.a));
 });
 test(`${name}: the omp pane prints every captured transcript item, in order, nothing missing`, () => {
  const rows = cues.filter(c => c.p === 'b' && (c.k === 'static' || c.k === 'event')).map(c => c.line);
  assert.equal(plain(rows[0]), `~/work/alpha ❯ glen tail -f ${session}`);
  const header = tailHeader(cols.b);
  assert.deepEqual(rows.slice(1, 1 + header.length), header);
  assert.equal(plain(rows[1 + header.length]), `following ${session} · Ctrl-C to stop`);
  const shown = rows.slice(2 + header.length).filter(line => plain(line) !== '');
  assert.deepEqual(shown, items(cols.b).flat());
 });
 test(`${name}: the agent sends the message the transcript shows`, () => {
  const sent = items(cols.b).findLast(item => plain(item[0]).startsWith('❯ user'));
  assert.equal(message, sent.slice(1).map(l => plain(l).replace(/^ {2}/, '')).join(' '));
  const typed = cues.filter(c => c.k === 'cmd' && c.p === 'a').map(c => c.text).join('');
  assert.ok(typed.endsWith(`glen send ${session} "${message}"`), typed);
 });
}

test('every capture decodes, and each tone it uses has a terminal colour class', () => {
 const css = readFileSync(fileURLToPath(new URL('./terminal.css', import.meta.url)), 'utf8');
 const tones = new Set(Object.keys(capture.captures).flatMap(key => lines(key).flat().map(span => span.tone).filter(Boolean)));
 assert.ok(tones.size >= 4, [...tones].join(' '));
 for (const tone of tones) assert.match(css, new RegExp(`\\.c-${tone}\\s*\\{`), `.c-${tone}`);
 assert.throws(() => lines('nope@1'), /no demo capture nope@1; add it to tests\/site_demo\.rs/);
});

test('the capture is glen’s ls with the demo sessions, the selection its reply line', () => {
 const header = w => plain(lsTable(w)[0]).split(/\s+/).filter(Boolean);
 assert.deepEqual(header(76), ['handle', 'state', 'title', 'turn', 'updated', 'model']);
 assert.deepEqual(header(44), ['handle', 'state', 'title', 'updated']);
 assert.equal(plain(lsTable(76).at(-1)), '3 sessions · 1 live');
 assert.equal(selection, '↩ Added the index. All 12 tests pass.');
});

test('the selection covers the reply line', () => {
 assert.ok(wide.some(c => c.p === 'a' && c.line && plain(c.line).startsWith(selection)));
 assert.ok(wide.some(c => c.k === 'sel' && c.p === 'a'));
});
