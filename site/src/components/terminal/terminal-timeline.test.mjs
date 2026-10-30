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

for (const replacesRow of [0,scenes[2].wide.length,scenes[2].wide.length+1]) test(`rejects transient replacesRow ${replacesRow}`,()=>{
 assert.throws(()=>compileTimeline([{...scenes[2],transient:{...scenes[2].transient,replacesRow}}]),/Scene 1: transient replacesRow/);
});
test('no transient means no beam or settle flash',()=>{
 const t=compileTimeline([scenes[0],scenes[1]]);
 assert.equal(t.beam,undefined);
 assert.equal(t.settleMs,undefined);
 assert.ok(t.rows.every(row=>row.endMs===undefined));
});
test('every output reveals after its command has finished typing',()=>{
 for (const row of timeline.rows.filter(row=>row.index>=0)) {
  const command=timeline.rows.find(command=>command.scene===row.scene && command.index===-1);
  assert.ok(row.startMs>=command.startMs+(command.type?.durationMs ?? 0));
  if (row.scene>0) assert.ok(row.startMs>command.startMs);
  assert.ok(timeline.revealCss.includes(`@keyframes ${row.name}{`));
  assert.ok(timeline.revealCss.includes(`${row.startMs/timeline.totalMs*100}%{visibility:visible;opacity:1}`));
 }
});
test('each reveal and scroll keyframe keeps the newest revealed rows in a contiguous window',()=>{
 const frames=[...timeline.css.matchAll(/([\d.]+)%\{transform:translateY\((?:calc\(-(\d+) \* var\(--t-row\)\)|0)\)\}/g)].map(match=>({time:Number(match[1])/100*timeline.totalMs,offset:Number(match[2] ?? 0)}));
 assert.equal(frames.length,timeline.scroll.length+2);
 const times=[...new Set([0,...timeline.rows.flatMap(row=>[row.startMs,...(row.endMs===undefined ? [] : [row.endMs])]),...timeline.scroll.map(step=>step.time),timeline.totalMs])];
 for (const time of times) {
  const revealed=timeline.rows.filter(row=>row.startMs<=time && row.units>0);
  const units=revealed.reduce((sum,row)=>sum+row.units,0);
  const offset=frames.findLast(frame=>frame.time<=time+.000001)?.offset ?? 0;
  assert.equal(offset,Math.max(0,units-WINDOW_ROWS));
  // Older history scrolls out; every retained row fits, with no hidden gaps in the tail.
  let position=0;
  for (const row of revealed) {
   const bottom=position+row.units;
   if (bottom>offset) {
    assert.ok(position-offset>=0);
    assert.ok(bottom-offset<=WINDOW_ROWS);
    if (row.endMs!==undefined && time>=row.endMs) {
     const replacement=timeline.rows.find(next=>next.scene===row.scene && next.index===row.index+1);
     assert.equal(replacement.startMs,row.endMs);
    }
   }
   position=bottom;
  }
 }
});
test('scroll ends at units minus the window and percentages strictly increase, including simultaneous reveals',()=>{
 for (const [t,windowRows] of [[timeline,WINDOW_ROWS],[compileTimeline([scenes[0]],{windowRows:2}),2]]) {
  const units=t.rows.reduce((sum,row)=>sum+row.units,0);
  assert.equal(t.finalOffset,units-windowRows);
  const percentages=[...t.css.matchAll(/([\d.]+)%\{/g)].map(match=>Number(match[1]));
  assert.equal(percentages[0],0);
  assert.equal(percentages.at(-1),100);
  percentages.forEach((percent,index)=>assert.ok(index===0 || percent>percentages[index-1]));
  assert.match(t.css,new RegExp(`100%\\{transform:translateY\\(calc\\(-${t.finalOffset} \\* var\\(--t-row\\)\\)\\)`));
 }
});
