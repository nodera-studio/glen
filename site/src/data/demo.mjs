// Synthetic demo written from docs/design.md (W0 contracts). Regenerate from real `glen` output against tests/fixtures in W7/W8 (D8).
/** @typedef {{text: string, tone?: 'muted'|'accent'|'accent_2'|'success'|'warning'|'danger'|'focus', glyph?: true}} Span */
/** @typedef {Span[]} Row */
/** @typedef {{cmd: string, wide: Row[], narrow: Row[], transient?: {replacesRow: number, label: string}}} Scene */
/** @param {string} text @param {Span['tone']} [tone] @returns {Row} */
const row = (text, tone) => text.split(/([●○✓])/).filter(Boolean).map(text => ({ text, tone, ...(/^[●○✓]$/.test(text) ? { glyph: /** @type {true} */ (true) } : {}) }));
export const cwd = '~/work/alpha';
/** @type {Scene[]} */
export const scenes = [
 { cmd: 'glen ls', wide: [
  row('HANDLE   TITLE           STATUS  LAST TURN  UPDATED  MODEL', 'muted'),
  [{text:'e27c0de  Fix migration   '},{text:'●',tone:'success',glyph:true},{text:' live  12         2s ago   opus',tone:'success'}],
  row('b7e2d81  Review routes   ○ idle  8          3m ago   sonnet', 'muted'),
  row('b8e4a07  Add fixtures    ○ idle  5          1h ago   codex', 'muted'),
 ], narrow: [
  row('HANDLE   TITLE           STATUS','muted'),
  [{text:'e27c0de  Fix migrat…     '},{text:'●',tone:'success',glyph:true},{text:' live',tone:'success'}],
  row('b7e2d81  Review rou…     ○ idle','muted'),
  row('b8e4a07  Add fixtur…     ○ idle','muted'),
 ] },
 { cmd: 'glen show e27c0de', wide: [
  [{text:'user       ',tone:'accent'},{text:'Check migration 0012.'}],
  [{text:'assistant  ',tone:'accent_2'},{text:'I will run the fixture tests.'}],
  row('tool       cargo test migration_0012','muted'),
  [{text:'assistant  ',tone:'accent_2'},{text:'Found a missing index.'}],
 ], narrow: [
  [{text:'user       ',tone:'accent'},{text:'Check migration 0012.'}],
  [{text:'assistant  ',tone:'accent_2'},{text:'Running fixture tests.'}],
  row('tool       cargo test migration_0012','muted'),
  [{text:'assistant  ',tone:'accent_2'},{text:'Missing an index.'}],
 ] },
 { cmd: 'glen send e27c0de "Fix 0012 and tell me when it passes."',
  transient: { replacesRow: 2, label: 'waiting for the reply' },
  wide: [
   [{text:'✓',tone:'success',glyph:true},{text:' delivered to e27c0de (live route)  job j3c9f41a'}],
   row('waiting for the reply','muted'),
   [{text:'✓',tone:'success',glyph:true},{text:' completed in 41s',tone:'success'}],
   [{text:'glen_reply ',tone:'accent_2'},{text:'Added the index. All 12 tests pass.'}],
   row('           Migration 0012 is ready.'),
  ], narrow: [
   row('✓ delivered e27c0de live j3c9f41a'),
   row('waiting for the reply','muted'),
   row('✓ completed in 41s','success'),
   [{text:'glen_reply ',tone:'accent_2'},{text:'All 12 tests pass.'}],
   row('           0012 is ready.'),
  ] },
];
