// @ts-check
// Decodes glen's `--color always` stdout (src/render/palette.rs, 24-bit SGR) into demo lines.
// Strict on purpose: an SGR, colour or escape it does not know throws, because a silent
// mis-decode would let the site drift from the CLI unnoticed.

/** @typedef {import('./demo.mjs').Span} Span */
/** @typedef {import('./demo.mjs').Line} Line */
/** @typedef {import('./demo.mjs').Tone} Tone */
/** @typedef {{tokens: {name: string, dark: {hex: string}}[]}} Palette */

const ESC = '\u001b';

/**
 * @param {string} ansi glen stdout with SGR escapes
 * @param {Palette} palette src/render/palette.json
 * @returns {Line[]}
 */
export function decode(ansi, palette) {
 /** @type {Map<string, Tone>} */
 const tones = new Map(palette.tokens.map(t => [t.dark.hex.toLowerCase(), /** @type {Tone} */ (t.name)]));
 const rows = ansi.split('\n');
 if (rows.length && rows[rows.length - 1] === '') rows.pop();
 /** @type {Tone|undefined} */
 let tone;
 let bold = false;
 return rows.map(row => {
  /** @type {Line} */
  const line = [];
  const push = (/** @type {string} */ text) => {
   if (!text) return;
   const last = line[line.length - 1];
   if (last && last.tone === tone && !!last.bold === bold) { last.text += text; return; }
   line.push({ text, ...(tone ? { tone } : {}), ...(bold ? { bold } : {}) });
  };
  let i = 0;
  while (i < row.length) {
   const at = row.indexOf(ESC, i);
   if (at < 0) { push(row.slice(i)); break; }
   push(row.slice(i, at));
   const m = /^\u001b\[([0-9;]*)m/.exec(row.slice(at));
   if (!m) throw new Error(`unsupported escape ${JSON.stringify(row.slice(at, at + 12))}`);
   const params = m[1] === '' ? ['0'] : m[1].split(';');
   for (let p = 0; p < params.length; p++) {
    const code = params[p];
    if (code === '0' || code === '') { tone = undefined; bold = false; }
    else if (code === '1') bold = true;
    else if (code === '22') bold = false;
    else if (code === '39') tone = undefined;
    else if (code === '38' && params[p + 1] === '2' && params.length >= p + 5) {
     const hex = '#' + params.slice(p + 2, p + 5).map(v => {
      const n = Number(v);
      if (!/^\d{1,3}$/.test(v) || n > 255) throw new Error(`bad colour in ${JSON.stringify(m[0])}`);
      return n.toString(16).padStart(2, '0');
     }).join('');
     const name = tones.get(hex);
     if (!name) throw new Error(`colour ${hex} in ${JSON.stringify(m[0])} is not a palette token`);
     tone = name;
     p += 4;
    } else throw new Error(`unsupported SGR ${JSON.stringify(m[0])}`);
   }
   i = at + m[0].length;
  }
  return line;
 });
}
