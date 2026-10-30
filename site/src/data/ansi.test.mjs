import { test } from 'node:test';
import assert from 'node:assert/strict';
import palette from '../../../src/render/palette.json' with { type: 'json' };
import { decode } from './ansi.mjs';

const E = '\u001b';
/** The 24-bit foreground SGR glen prints for a palette token (src/render/palette.rs). */
const fg = name => {
 const hex = palette.tokens.find(t => t.name === name).dark.hex;
 return `${E}[38;2;${[1, 3, 5].map(i => parseInt(hex.slice(i, i + 2), 16)).join(';')}m`;
};

test('bold, colour and reset decode to spans; a trailing newline adds no line', () => {
 const out = decode(`${fg('muted')}${E}[1mhandle${E}[0m${E}[0m  ${fg('focus')}e27c0de${E}[0m\nplain\n`, palette);
 assert.deepEqual(out, [
  [{ text: 'handle', tone: 'muted', bold: true }, { text: '  ' }, { text: 'e27c0de', tone: 'focus' }],
  [{ text: 'plain' }],
 ]);
});

test('only one trailing empty line is dropped; inner empty lines stay', () => {
 assert.deepEqual(decode('a\n\nb\n\n', palette), [[{ text: 'a' }], [], [{ text: 'b' }], []]);
});

test('adjacent spans with the same style merge, empty spans drop', () => {
 assert.deepEqual(decode(`${fg('muted')} · a${E}[0m${fg('muted')} · b${E}[0m${fg('success')}${E}[0m!`, palette), [[{ text: ' · a · b', tone: 'muted' }, { text: '!' }]]);
});

test('39 clears the tone and 22 clears bold; an empty SGR resets', () => {
 assert.deepEqual(decode(`${E}[1m${fg('accent')}x${E}[39my${E}[22mz${fg('danger')}${E}[1mw${E}[mv`, palette), [[
  { text: 'x', tone: 'accent', bold: true }, { text: 'y', bold: true }, { text: 'z' }, { text: 'w', tone: 'danger', bold: true }, { text: 'v' },
 ]]);
});

test('every palette token round-trips by its dark colour', () => {
 assert.equal(palette.tokens.length, 8);
 for (const { name } of palette.tokens) assert.deepEqual(decode(`${fg(name)}${name}${E}[0m`, palette), [[{ text: name, tone: name }]]);
});

test('anything glen does not print throws, naming the sequence', () => {
 assert.throws(() => decode(`${E}[38;2;1;2;3mx`, palette), /#010203.*not a palette token/);
 assert.throws(() => decode(`${E}[38;5;250mx`, palette), /unsupported SGR/);
 assert.throws(() => decode(`${E}[4mx`, palette), /unsupported SGR/);
 assert.throws(() => decode(`${E}[2Kx`, palette), /unsupported escape/);
 assert.throws(() => decode(`${E}]8;;https://x${E}\\`, palette), /unsupported escape/);
});
