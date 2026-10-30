// Synthetic sessions, time compressed. W7/W8: regenerate from real glen output against tests/fixtures (D8).
/** @typedef {{text:string,tone?:string,bold?:boolean}} Span */
/** @typedef {{id:string,p:'a'|'b',k:string,at:number,spans:Span[],cmd?:string,start?:number,end?:number}} DemoRow */
const span = (text, tone, bold=false) => ({text,tone,bold});
const prompt = [span('~/work/alpha','accent_2'),span(' ❯ ','accent')];
/** @type {DemoRow[]} */
export const script = [];
const add = (id,p,k,at,...spans) => script.push({id,p,k,at,spans});
add('ls','a','cmd',0,...prompt,span('glen ls'));
Object.assign(script.at(-1),{cmd:'glen ls',start:420,end:1180});
add('head','a','flush',1320,span('handle   state   title                 turn      updated  model','muted',true));
add('live','a','flush',1336,span('e27c0de','focus'),span('  ● live','success'),span('  Fix migration 0012    complete  2s ago   claude-sonnet-4-5'));
add('idle1','a','flush',1352,span('b7e2d81','focus'),span('  ○ idle  Review routes         complete  3m ago   claude-sonnet-4-5'));
add('idle2','a','flush',1368,span('b8e4a07','focus'),span('  ○ idle  Add fixtures          complete  1h ago   claude-sonnet-4-5'));
add('blank1','a','flush',1384,span(''));
add('count','a','flush',1400,span('3 sessions · 1 live','muted'));
add('send','a','cmd',1420,...prompt,span('glen send e27c0de "Fix 0012 and tell me when it passes."'));
Object.assign(script.at(-1),{cmd:'glen send e27c0de "Fix 0012 and tell me when it passes."',start:2000,end:4700});
add('completed','a','flush',10760,span('✓ completed','success'),span(' · j3c9f41 · e27c0de · 41s','muted'));
add('blank2','a','flush',10776,span(''));
add('answer','a','flush',10792,span('  Added the index. All 12 tests pass. Migration 0012 is ready.'));
add('reply','a','flush',10808,span('  ↩ Added the index. All 12 tests pass.','muted'),span(' ✓','success'));
add('prompt','a','prompt',14000,...prompt);
add('tail','b','static',0,...prompt,span('glen tail -f e27c0de'));
add('heading','b','static',0,span('Fix migration 0012',undefined,true));
add('meta','b','static',0,span('e27c0de · ~/work/alpha · claude-sonnet-4-5 · ','muted'),span('● live','success'));
add('blank3','b','static',0,span(''));
add('assistant1','b','static',0,span('◆ assistant','accent_2',true),span(' · claude-sonnet-4-5 · 12:01','muted'));
add('found','b','static',0,span('  Found a missing index on users.email.'));
add('user','b','event',5300,span('❯ user','accent',true),span(' · 12:04','muted'));
add('request','b','event',5340,span('  Fix 0012 and tell me when it passes.'));
add('assistant2','b','event',6100,span('◆ assistant','accent_2',true),span(' · 12:04','muted'));
add('thinking','b','event',6140,span('  ∴ thinking · 18 words','muted'));
add('edit','b','event',7000,span('  ↳ edit'),span(' migrations/0012_add_index.sql','muted'),span(' · '),span('✓','success'),span(' 3 lines'));
add('bash','b','event',8900,span('  ↳ bash'),span(' cargo test migration_0012','muted'),span(' · '),span('✓','success'),span(' 14 lines'));
add('delivered','b','event',10200,span('  ↳ glen_reply'),span(' Added the index. All 12 tests pass.','muted'),span(' · '),span('✓','success'),span(' delivered'));
add('final','b','event',10900,span('  Added the index. All 12 tests pass. Migration 0012 is ready.'));
