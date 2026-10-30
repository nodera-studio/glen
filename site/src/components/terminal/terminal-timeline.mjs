// @ts-check
export const TYPE_MS_PER_CHAR = 32;
export const PRE_OUTPUT_MS = 280;
export const LINE_STAGGER_MS = 45;
export const SCENE_GAP_MS = 1100;
export const SPINNER_FRAME_MS = 80;
export const SPINNER_FRAMES = '⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏';
export const SEND_WAIT_MS = 2400;
export const WINDOW_ROWS = 14;
// Split commands without changing a character; narrow lines type sequentially.
/** @param {string} command */
export function commandParts(command) {
 /** @type {{text: string, offset: number}[]} */
 const parts = [];
 let offset = 0;
 while (offset < command.length) {
  let length = Math.min(35, command.length - offset);
  if (offset + length < command.length) {
   const space = command.slice(offset, offset + length).lastIndexOf(' ');
   if (space > 0) length = space + 1;
  }
  parts.push({ text: command.slice(offset, offset + length), offset }); offset += length;
 }
 return parts;
}

/** @typedef {import('../../data/demo.mjs').Scene} Scene */
/** @typedef {{scene: number, index: number, startMs: number, endMs?: number, units: number, type?: {steps: number, durationMs: number}}} TimedRow */
/** @param {Scene[]} scenes @param {{windowRows?: number}} [opts] */
export function compileTimeline(scenes, opts = {}) {
 const windowRows = opts.windowRows ?? WINDOW_ROWS;
 /** @type {TimedRow[]} */
 const rows = [];
 let time = 0;
 /** @type {{startMs: number, durationMs: number, laps: number} | undefined} */
 let beam;
 for (const [scene, data] of scenes.entries()) {
  if (data.wide.length !== data.narrow.length) throw new Error(`Scene ${scene + 1}: wide/narrow row-count mismatch`);
  if (data.transient && (data.transient.replacesRow < 1 || data.transient.replacesRow >= data.wide.length)) throw new Error(`Scene ${scene + 1}: transient replacesRow must be between 1 and ${data.wide.length - 1}`);
  for (const variant of /** @type {const} */ (['wide','narrow'])) {
   data[variant].forEach((row, index) => {
    if ([...row.map(s => s.text).join('')].length > (variant === 'wide' ? 76 : 38)) throw new Error(`Scene ${scene + 1} ${variant} row ${index + 1} exceeds columns`);
   });
  }
  const durationMs = scene === 0 ? 0 : Math.min(1400, Math.max(300, data.cmd.length * TYPE_MS_PER_CHAR));
  // Reserve wrapped command rows in both variants, preserving shared scroll geometry.
  rows.push({ scene, index: -1, startMs: time, units: Math.max(1, commandParts(data.cmd).length), ...(scene ? { type: { steps: data.cmd.length, durationMs } } : {}) });
  let output = scene === 0 ? 0 : time + durationMs + PRE_OUTPUT_MS;
  for (let index = 0; index < data.wide.length; index++) {
   let startMs = output;
   if (data.transient && index === data.transient.replacesRow) {
    const spinner = rows[rows.length - 1];
    startMs = spinner.startMs + SEND_WAIT_MS;
    spinner.endMs = startMs;
    beam = { startMs: spinner.startMs, durationMs: SEND_WAIT_MS, laps: 2 };
   }
   rows.push({ scene, index, startMs, units: data.transient && index === data.transient.replacesRow ? 0 : 1 });
   output = scene === 0 ? 0 : startMs + LINE_STAGGER_MS;
  }
  time = output + SCENE_GAP_MS;
 }
 const settleMs = beam ? beam.startMs + beam.durationMs : undefined;
 rows.push({ scene: scenes.length, index: -2, startMs: time - SCENE_GAP_MS + LINE_STAGGER_MS, units: 1 });
 const totalMs = Math.max(time, settleMs === undefined ? 0 : settleMs + 1300);
 // Reveal, replacement and scroll all use the same clock, with steps at row boundaries.
 const timedRows = rows.map((row, index) => ({ ...row, name: `t-row-${index}` }));
 const revealCss = timedRows.map(row => {
  const frames = row.startMs ? ['0%{visibility:hidden;opacity:0}'] : [];
  frames.push(`${row.startMs / totalMs * 100}%{visibility:visible;opacity:1}`);
  if (row.endMs !== undefined) frames.push(`${row.endMs / totalMs * 100}%{visibility:hidden;opacity:0}`);
  frames.push(`100%{visibility:${row.endMs === undefined ? 'visible' : 'hidden'};opacity:${row.endMs === undefined ? 1 : 0}}`);
  return `@keyframes ${row.name}{${frames.join('')}}`;
 }).join('');
 let count = 0;
 /** @type {{time: number, offset: number}[]} */
 const scroll = [];
 for (const row of rows) {
  count += row.units;
  if (count > windowRows && row.units) {
   const previous = scroll[scroll.length - 1];
   if (previous?.time === row.startMs) previous.offset = count - windowRows;
   else scroll.push({ time: row.startMs, offset: count - windowRows });
  }
 }
 const finalOffset = Math.max(0, count - windowRows);
 let css = '';
 if (scroll.length) {
  const frames = scroll[0].time ? ['0%{transform:translateY(0)}'] : [];
  for (const step of scroll) {
   const percent = step.time / totalMs * 100;
   frames.push(`${percent}%{transform:translateY(calc(-${step.offset} * var(--t-row)))}`);
  }
  frames.push(`100%{transform:translateY(calc(-${finalOffset} * var(--t-row)))}`);
  css = `@keyframes t-scroll{${frames.join('')}}`;
 }
 return { rows: timedRows, beam, settleMs, totalMs, finalOffset, scroll, revealCss, css };
}
