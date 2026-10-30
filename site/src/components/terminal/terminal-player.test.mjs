import { test } from 'node:test';
import assert from 'node:assert/strict';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { plan, total, paneAt, typeDelays, mulberry32, SPINNER_TICKS, TICK_MS, ROW_KINDS, CUE_KINDS, TYPE } from './terminal-player.mjs';
import { script, COLS, ROWS, WINDOWS, SEED, lsTable, wrap, plain, selection } from '../../data/demo.mjs';

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
  assert.ok(offset >= 0);
  assert.ok(on.length - 1 - offset < rows, `${pane}@${t}: newest row ${on.length - 1} scrolled out (offset ${offset})`);
  assert.ok(on.length - offset <= rows, `${pane}@${t}: cursor line below the pane`);
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
 assert.ok(done && plain(done.line).startsWith('✓ completed · j3c9f41 · e27c0de · '));
 const send = wide.find(c => c.k === 'cmd' && c.text.startsWith('glen send'));
 assert.ok(spin.at >= send.commit, 'spinner starts after Enter');
 const reply = wide.find(c => c.k === 'event' && plain(c.line).startsWith('  ↳ glen_reply'));
 const hop = wide.find(c => c.k === 'hop');
 assert.ok(hop.at >= reply.at && hop.at < spin.end, 'the reply hops across before glen prints it');
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

test('glen ls drops columns the way ls.rs does', () => {
 const header = w => plain(lsTable(w)[0]).split(/\s+/).filter(Boolean);
 assert.deepEqual(header(76), ['handle', 'state', 'title', 'turn', 'updated', 'model']);
 assert.deepEqual(header(44), ['handle', 'state', 'title', 'updated']);
 assert.equal(plain(lsTable(76).at(-1)), '3 sessions · 1 live');
 for (const w of [44, 76]) for (const line of lsTable(w)) assert.ok([...plain(line)].length <= w);
 assert.equal(plain(lsTable(76)[1]), 'e27c0de  ● live  Fix migration 0012  complete  2s ago   claude-sonnet-4-5');
});

test('text::wrap keeps the indent and breaks on words', () => {
 assert.deepEqual(wrap('  Added the index. All 12 tests pass. Migration 0012 is ready.', 44), ['  Added the index. All 12 tests pass.', '  Migration 0012 is ready.']);
 assert.deepEqual(wrap('short', 10), ['short']);
 assert.deepEqual(wrap('abcdefghij', 4), ['abcd', 'efgh', 'ij']);
});

test('the selection covers the reply line', () => {
 assert.ok(wide.some(c => c.p === 'a' && c.line && plain(c.line).startsWith(selection)));
 assert.ok(wide.some(c => c.k === 'sel' && c.p === 'a'));
});
