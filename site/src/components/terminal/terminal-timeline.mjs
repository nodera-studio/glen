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
 let beam = { startMs: 0, durationMs: 0, laps: 2 };
 for (const [scene, data] of scenes.entries()) {
  if (data.wide.length !== data.narrow.length) throw new Error(`Scene ${scene + 1}: wide/narrow row-count mismatch`);
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
 const settleMs = beam.startMs + beam.durationMs;
 rows.push({ scene: scenes.length, index: -2, startMs: time - SCENE_GAP_MS + LINE_STAGGER_MS, units: 1 });
 const totalMs = Math.max(time, settleMs + 1300);
 let count = 0;
 const scroll = rows.flatMap(row => {
  count += row.units;
  return count > windowRows && row.units ? [{ time: row.startMs, offset: count - windowRows }] : [];
 });
 const finalOffset = Math.max(0, count - windowRows);
 let css = '';
 if (scroll.length) {
  let previous = 0;
  const frames = ['0%{transform:translateY(0)}'];
  for (const step of scroll) {
   const percent = step.time / totalMs * 100;
   frames.push(`${Math.max(0,percent - .001).toFixed(4)}%{transform:translateY(calc(-${previous} * var(--t-row)))}`, `${percent.toFixed(4)}%{transform:translateY(calc(-${step.offset} * var(--t-row)))}`);
   previous = step.offset;
  }
  frames.push(`100%{transform:translateY(calc(-${finalOffset} * var(--t-row)))}`);
  css = `@keyframes t-scroll{${frames.join('')}}`;
 }
 return { rows, beam, settleMs, totalMs, finalOffset, css };
}
