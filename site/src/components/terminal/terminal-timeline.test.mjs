import { test } from 'node:test';
import assert from 'node:assert/strict';
import { compileTimeline, WINDOW_ROWS, commandParts, SEND_WAIT_MS } from './terminal-timeline.mjs';
import { scenes } from '../../data/demo.mjs';
const timeline = compileTimeline(scenes);
test('scene 1 is visible at first paint', () => assert.ok(timeline.rows.filter(r => r.scene === 0).every(r => r.startMs === 0)));
test('row timing never decreases', () => timeline.rows.forEach((r,i,a) => assert.ok(!i || r.startMs >= a[i-1].startMs)));
test('typing steps equal the command length', () => timeline.rows.filter(r=>r.type).forEach(r=>assert.equal(r.type.steps,scenes[r.scene].cmd.length)));
test('completion replaces the spinner at exactly its end', () => { const spinner = timeline.rows.find(r=>r.endMs); assert.equal(spinner.endMs,timeline.rows.find(r=>r.scene===spinner.scene && r.index===spinner.index+1).startMs); });
test('rejects mismatched variants', () => assert.throws(()=>compileTimeline([{cmd:'x',wide:[[{text:'a'}]],narrow:[]}]),/Scene 1.*wide\/narrow/));
for (const [variant,limit] of [['wide',76],['narrow',38]]) test(`rejects overlong ${variant} row`,()=>assert.throws(()=>compileTimeline([{cmd:'x',wide:[[{text:'a'}]],narrow:[[{text:'a'}]],[variant]:[[{text:'x'.repeat(limit+1)}]]}]),new RegExp(`Scene 1 ${variant}`)));
test('does not emit scrolling for a short transcript',()=>{ const t=compileTimeline([scenes[0]],{windowRows:WINDOW_ROWS}); assert.equal(t.css,''); });
test('real demo compiles within fourteen seconds',()=>{ assert.ok(timeline.totalMs<=14000); assert.match(timeline.css,/@keyframes t-scroll/); });

test('command wrapping preserves every ASCII character and the shared row geometry',()=>{
 for (const [index,scene] of scenes.entries()) {
  const parts=commandParts(scene.cmd);
  assert.equal(parts.map(p=>p.text).join(''),scene.cmd);
  assert.ok(parts.every(p=>p.text.length<=35));
  assert.match(scene.cmd,/^[\x20-\x7E]+$/);
  assert.equal(timeline.rows.find(r=>r.scene===index && r.index===-1).units,parts.length);
 }
});
test('row width limits are inclusive',()=>{
 assert.doesNotThrow(()=>compileTimeline([{cmd:'x',wide:[[{text:'x'.repeat(76)}]],narrow:[[{text:'x'.repeat(38)}]]}]));
});

test('send beam and spinner run for the wait and settle at completion',()=>{
 const spinner=timeline.rows.find(r=>r.endMs);
 assert.equal(spinner.endMs-spinner.startMs,SEND_WAIT_MS);
 assert.equal(timeline.beam.startMs,spinner.startMs);
 assert.equal(timeline.settleMs,spinner.endMs);
});
