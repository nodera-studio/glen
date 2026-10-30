// @ts-check
// The hero terminal player. `plan()` runs at build time (Terminal.astro) and in unit tests: it
// turns the demo script into timed cues. `boot()` is minified with esbuild and inlined after the
// figure; it reads the timed rows back from the DOM and drives them from one rAF clock.

/** indicatif 0.18 default tick_strings (style.rs:82) without the final "finished" blank. */
export const SPINNER_TICKS = '⠁⠁⠉⠙⠚⠒⠂⠂⠒⠲⠴⠤⠄⠄⠤⠠⠠⠤⠦⠖⠒⠐⠐⠒⠓⠋⠉⠈⠈';
/** glen's spinner ticks every 80 ms (src/render/spinner.rs). */
export const TICK_MS = 80;
/** Row kinds stamped as data-k. */
export const ROW_KINDS = ['static', 'cmd', 'flush', 'event', 'spin', 'prompt'];
/** Non-row cue kinds carried in data-cues. */
export const CUE_KINDS = ['title', 'msg', 'focus', 'hop', 'sel', 'done'];
/** Typing model, ms. */
export const TYPE = { base: 40, lo: -14, hi: 18, space: 45, punct: 30, glanceAt: 12, glance: 140, glanceMin: 30, enter: 240 };
export const FLUSH_LINE_MS = 16;
export const EVENT_LINE_MS = 40;
export const SEL = { perChar: 12, hold: 1400, fade: 200 };
export const HOP_MS = 520;

/** @param {number} seed */
export function mulberry32(seed) {
 let a = seed >>> 0;
 return () => {
  a = (a + 0x6d2b79f5) >>> 0;
  let t = a;
  t = Math.imul(t ^ (t >>> 15), t | 1);
  t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
  return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
 };
}

/**
 * Delay before each character (index 0 is the first key, so 0).
 * @param {string} text @param {() => number} rng
 */
export function typeDelays(text, rng) {
 return [...text].map((_, i) => {
  if (!i) return 0;
  const prev = text[i - 1];
  let d = TYPE.base + TYPE.lo + rng() * (TYPE.hi - TYPE.lo);
  if (prev === ' ') d += TYPE.space;
  else if ('-/"'.includes(prev)) d += TYPE.punct;
  if (i === TYPE.glanceAt && text.length >= TYPE.glanceMin) d += TYPE.glance;
  return Math.round(d);
 });
}

/** @typedef {import('../../data/demo.mjs').Step} Step */
/** @typedef {import('../../data/demo.mjs').Line} Line */
/**
 * @typedef {{k: string, at: number, p?: 'a'|'b', v?: string, line?: Line, text?: string,
 *  ty?: number[], commit?: number, end?: number, id?: string}} Cue
 */

/**
 * Resolve the script into cues. Row cues come in pane order; `narrow` turns the reply hop into a
 * tab switch, because only one pane shows.
 * @param {Step[]} script
 * @param {{seed?: number, narrow?: boolean, cols?: {a: number, b: number}, promptWidth?: number}} [opts]
 * @returns {Cue[]}
 */
export function plan(script, { seed = 7, narrow = false, cols = narrow ? { a: 44, b: 44 } : { a: 76, b: 64 }, promptWidth = 15 } = {}) {
 const rng = mulberry32(seed);
 /** @type {Cue[]} */
 const cues = [];
 /** @type {Record<string, number>} */
 const ends = {};
 let prev = 0;
 for (const step of script) {
  const at = (step.after ? ends[step.after] : prev) + (step.dt ?? 0);
  let end = at;
  const p = step.p;
  if (step.k === 'cmd' || step.k === 'prompt') {
   const text = step.text ?? '';
   const delays = typeDelays(text, rng);
   let key = at + (step.think ?? 0);
   const times = delays.map(d => (key += d));
   end = text ? times[times.length - 1] + TYPE.enter : at;
   // The shell hard-wraps the command line at the pane's column count.
   let offset = 0, row = 0;
   do {
    const room = cols[p ?? 'a'] - (row ? 0 : promptWidth);
    const slice = text.slice(offset, offset + room);
    cues.push({ k: step.k, p, at: row ? times[offset] : at, text: slice, ty: times.slice(offset, offset + slice.length), commit: end, ...(row ? {} : { line: [] }) });
    offset += room; row++;
   } while (offset < text.length);
  } else if (step.k === 'static' || step.k === 'flush' || step.k === 'event') {
   const lines = step.lines?.(cols[p ?? 'a']) ?? [];
   const gap = step.k === 'flush' ? FLUSH_LINE_MS : step.k === 'event' ? EVENT_LINE_MS : 0;
   lines.forEach((line, i) => cues.push({ k: step.k, p, at: at + i * gap, line }));
   end = at + Math.max(0, lines.length - 1) * gap;
   if (step.ends) { const spin = cues.find(c => c.id === step.ends); if (spin) spin.end = at; }
  } else if (step.k === 'spin') {
   cues.push({ k: 'spin', p, at, v: step.v, id: step.id });
  } else if (step.k === 'hop' && narrow) {
   cues.push({ k: 'focus', p: 'a', at });
  } else {
   cues.push({ k: step.k, at, ...(p ? { p } : {}), ...(step.v ? { v: step.v } : {}) });
  }
  if (step.id) ends[step.id] = end;
  prev = end;
 }
 return cues;
}

/** Duration of a plan: the `done` cue. @param {Cue[]} cues */
export const total = cues => Math.max(...cues.filter(c => c.k === 'done').map(c => c.at));

/**
 * Rows a pane shows at time t, newest last, and the scroll offset that keeps the newest row and
 * the cursor line inside `rows`. Mirrors boot()'s render loop for the unit tests.
 * @param {Cue[]} cues @param {'a'|'b'} pane @param {number} t @param {number} rows
 */
export function paneAt(cues, pane, t, rows) {
 const list = cues.filter(c => c.p === pane && ROW_KINDS.includes(c.k));
 const on = list.filter(c => c.at <= t && !(c.end !== undefined && c.end <= t));
 const last = on[on.length - 1];
 const open = pane === 'a' && last && (last.k === 'cmd' || last.k === 'prompt') && !(last.commit !== undefined && last.k === 'cmd' && last.commit <= t);
 const below = pane === 'b' || (!open && last?.k !== 'spin') ? 1 : 0;
 return { on, offset: Math.max(0, on.length + below - rows) };
}

/** @param {HTMLElement | null} root */
export function boot(root) {
 if (!root) return;
 const win = /** @type {HTMLElement} */ (root.querySelector('.term-win'));
 const set = (/** @type {string} */ k, /** @type {string} */ v) => root.setAttribute('data-' + k, v);
 const reduce = matchMedia('(prefers-reduced-motion: reduce)');
 if (reduce.matches) return set('term', 'static');
 /** @type {[number, string, string?][]} */
 const cues = JSON.parse(root.dataset.cues || '[]');
 const end = Math.max(...cues.filter(c => c[1] == 'done').map(c => c[0]));
 const num = (/** @type {Element} */ el, /** @type {string} */ k) => { const v = el.getAttribute('data-' + k); return v == null ? Infinity : +v; };
 const variants = [...root.querySelectorAll('.term-v')].map(v => ({
  el: /** @type {HTMLElement} */ (v),
  panes: [...v.querySelectorAll('.term-pane')].map(pane => {
   let cur = pane.querySelector('.cur');
   if (!cur) { cur = document.createElement('i'); cur.className = 'cur'; cur.setAttribute('aria-hidden', 'true'); }
   return {
    p: pane.getAttribute('data-p'), cur, off: -1, max: 0, host: /** @type {Element|null|undefined} */ (undefined),
    scroll: /** @type {HTMLElement} */ (pane.querySelector('.term-scroll')),
    spare: /** @type {HTMLElement} */ (pane.querySelector('.spare')),
    rows: [...pane.querySelectorAll('.tl[data-at]')].map(el => {
     const ty = el.querySelector('.ty'), sm = el.querySelector('.sm');
     return { el, at: num(el, 'at'), end: num(el, 'end'), commit: num(el, 'commit'), k: el.getAttribute('data-k'), on: false, n: -1, f: -1,
      ty, text: ty?.textContent ?? '', times: (el.getAttribute('data-ty') || '').split(',').filter(Boolean).map(Number),
      sp: el.querySelector('.sp'), sm, sv: sm?.textContent ?? '', mark: /** @type {HTMLElement|null} */ (el.querySelector('.sel')) };
    }),
   };
  }),
 }));
 const tabs = [...root.querySelectorAll('.term-tab')];
 const title = root.querySelector('.term-proc');
 const pauseBtn = /** @type {HTMLButtonElement} */ (root.querySelector('.term-pause'));
 const replayBtn = /** @type {HTMLButtonElement} */ (root.querySelector('.term-replay'));
 const dot = /** @type {HTMLElement} */ (root.querySelector('.term-dot'));
 const marks = [...new Set(cues.map(c => c[0]).concat(variants.flatMap(v => v.panes.flatMap(p => p.rows.flatMap(r => [r.at, r.end, r.commit])))))].filter(isFinite).sort((a, b) => a - b);
 const q = (new URLSearchParams(location.search).get('term') || '').split(':');
 const speed = q[0] == 'fast' ? 20 : 1;
 let t = 0, raf = 0, last = 0, mode = '', userPaused = false, seen = false, visible = false;
 /** The 600 ms autoplay delay. @type {ReturnType<typeof setTimeout>|undefined} */
 let start;
 /** @type {string|null} */
 let userFocus = null;
 /** @type {{x: number, y: number, a: number, d: number}|null} */
 let hop = null;
 let active = variants[0], narrow = false, mark = -1;
 const lastCue = (/** @type {string} */ k) => { let v; for (const c of cues) if (c[0] <= t && (c[1] == k || (narrow && k == 'focus' && c[1] == 'hop'))) v = c[1] == 'hop' ? 'a' : c[2]; return v; };
 const measure = () => {
  active = variants.find(v => v.el.offsetParent) || variants[0];
  narrow = getComputedStyle(/** @type {Element} */ (win.querySelector('.term-body'))).getPropertyValue('--narrow').trim() == '1';
  hop = null;
  for (const v of variants) for (const p of v.panes) { p.off = -1; p.host = undefined; p.max = +getComputedStyle(p.scroll).getPropertyValue('--rows') || 99; }
 };
 const render = () => {
  const focus = userFocus || lastCue('focus') || 'a';
  set('focus', focus);
  tabs.forEach(tab => { const on = tab.getAttribute('data-p') == focus; tab.setAttribute('aria-selected', '' + on); tab.setAttribute('tabindex', on ? '0' : '-1'); });
  if (title) { const v = '~/work/alpha — ' + (lastCue('title') || 'zsh'); if (title.textContent != v) title.textContent = v; }
  const msg = lastCue('msg');
  const sel = cues.find(c => c[1] == 'sel');
  let typing = false;
  for (const pane of active.panes) {
   let n = 0, /** @type {any} */ top = null, /** @type {any} */ next = null;
   for (const r of pane.rows) {
    const on = r.at <= t && !(r.end <= t);
    if (on != r.on) { r.on = on; r.el.classList.toggle('on', on); r.el.classList.toggle('off', r.end <= t); }
    if (on) { n++; top = r; } else if (!next && !(r.end <= t)) next = r;
    if (r.ty) {
     let c = 0;
     while (c < r.times.length && r.times[c] <= t) c++;
     if (c != r.n) { r.n = c; r.ty.textContent = r.text.slice(0, c); }
     if (r.times.length && r.times[0] <= t && t <= r.times[r.times.length - 1] + 500) typing = true;
    }
    if (r.sp && on) {
     const f = Math.floor((t - r.at) / TICK_MS) % SPINNER_TICKS.length;
     if (f != r.f) { r.f = f; r.sp.textContent = SPINNER_TICKS[f]; }
     // Before the first message cue the spinner shows its start message (also after Replay).
     if (r.sm && r.sm.textContent != (msg || r.sv)) r.sm.textContent = msg || r.sv;
    }
    if (r.mark) {
     const d = sel ? t - sel[0] : -1;
     const len = +(r.mark.getAttribute('data-n') || 0);
     const hold = len * SEL.perChar + SEL.hold;
     r.mark.style.width = d < 0 || d > hold + SEL.fade ? '0' : Math.min(len, Math.ceil(d / SEL.perChar)) + 'ch';
     r.mark.style.opacity = d > hold ? '' + Math.max(0, 1 - (d - hold) / SEL.fade) : '1';
    }
   }
   const open = pane.p == 'a' && top && (top.k == 'prompt' || (top.k == 'cmd' && top.commit > t));
   const host = pane.p == 'a' && top?.k == 'spin' ? null : open ? top.el : (next ? next.el : pane.spare);
   if (host !== pane.host) { pane.host = host; if (host) open ? host.append(pane.cur) : host.prepend(pane.cur); else pane.cur.remove(); }
   pane.cur.classList.toggle('hollow', pane.p != focus);
   pane.cur.classList.toggle('solid', typing && pane.p == 'a');
   pane.cur.classList.toggle('fin', t >= end);
   // The cursor line takes a row below the newest one, except on an open command line or the
   // spinner, which hold the cursor themselves (paneAt mirrors this).
   const off = Math.max(0, n + (open || host === null ? 0 : 1) - pane.max);
   if (off != pane.off) { pane.off = off; pane.scroll.style.transform = off ? `translateY(${-off}lh)` : ''; }
  }
  // The reply crosses from the omp pane to the agent's spinner (wide only).
  const h = cues.find(c => c[1] == 'hop');
  const d = h ? t - h[0] : -1;
  if (!narrow && d >= 0 && d < HOP_MS) {
   if (!hop) {
    const box = /** @type {HTMLElement} */ (dot.parentElement).getBoundingClientRect();
    const rect = (/** @type {string} */ sel) => active.el.querySelector(sel)?.getBoundingClientRect();
    const a = rect('[data-p=b] [data-hop]'), b = rect('[data-p=a] .tl.on[data-k=spin] .sp');
    if (a && b) hop = { x: a.left - box.left + 24, y: a.top + a.height / 2 - box.top, a: b.left + b.width / 2 - box.left, d: b.top + b.height / 2 - box.top };
   }
   if (hop) {
    const e = 1 - Math.pow(1 - d / HOP_MS, 4), x = hop.x + (hop.a - hop.x) * e, y = hop.y + (hop.d - hop.y) * e;
    dot.style.transform = `translate(${x}px,${y}px) rotate(${Math.atan2(hop.d - hop.y, hop.a - hop.x)}rad)`;
    dot.style.opacity = '1';
   }
  } else dot.style.opacity = '0';
  let m = 0;
  for (const x of marks) if (x <= t) m = x;
  if (m != mark) { mark = m; set('term-t', '' + m); }
 };
 const state = (/** @type {string} */ s) => {
  mode = s; set('term', s);
  pauseBtn.setAttribute('aria-pressed', '' + (s == 'paused' && userPaused));
  pauseBtn.disabled = s == 'done';
 };
 const frame = (/** @type {number} */ now) => {
  t = Math.min(end, t + Math.min(now - last, 50) * speed);
  last = now;
  render();
  if (t >= end) state('done'); else raf = requestAnimationFrame(frame);
 };
 const play = () => {
  if (mode == 'playing' || mode == 'done' || mode == 'static' || userPaused || !visible || document.hidden) return;
  state('playing'); last = performance.now(); raf = requestAnimationFrame(frame);
 };
 const pause = () => { cancelAnimationFrame(raf); if (mode == 'playing') state('paused'); };
 // Reduced motion turned on mid-run: stop the clock and settle every variant on the final frame,
 // the state a reduced-motion boot starts in. Turned off again, the frame stays and Replay returns.
 const still = () => {
  cancelAnimationFrame(raf); clearTimeout(start);
  t = end; userPaused = false; userFocus = null;
  const shown = active;
  for (const v of variants) { active = v; render(); }
  active = shown;
  state('static');
 };
 reduce.addEventListener('change', () => { if (reduce.matches) still(); else if (mode == 'static') state('done'); });
 pauseBtn.addEventListener('click', () => {
  userPaused = !userPaused;
  if (userPaused) { pause(); state('paused'); } else { userFocus = null; seen = visible = true; play(); }
 });
 replayBtn.addEventListener('click', () => {
  cancelAnimationFrame(raf); t = 0; userPaused = false; userFocus = null; mode = ''; seen = visible = true;
  render(); state('armed'); play();
 });
 tabs.forEach((tab, i) => {
  tab.addEventListener('click', () => { if (mode == 'static') return; userFocus = tab.getAttribute('data-p'); if (mode != 'done') { userPaused = true; pause(); state('paused'); } render(); });
  tab.addEventListener('keydown', e => {
   const k = /** @type {KeyboardEvent} */ (e).key;
   if (k != 'ArrowLeft' && k != 'ArrowRight' && k != 'Home' && k != 'End') return;
   e.preventDefault();
   const next = /** @type {HTMLElement} */ (tabs[k == 'Home' ? 0 : k == 'End' ? tabs.length - 1 : (i + 1) % tabs.length]);
   next.click(); next.focus();
  });
 });
 /** @type {ReturnType<typeof setTimeout>|undefined} */
 let resize;
 addEventListener('resize', () => { clearTimeout(resize); resize = setTimeout(() => { measure(); render(); }, 150); });
 root.querySelectorAll('.term-ctl button').forEach(b => b.removeAttribute('hidden'));
 measure();
 if (q[0] == 'end' || q[0] == 'at') {
  // A seek starts paused (or done); Play and Replay then run under the same observers.
  t = q[0] == 'end' ? end : Math.min(end, +q[1] || 0);
  userPaused = t < end;
  render(); state(t < end ? 'paused' : 'done');
 } else { render(); state('armed'); }
 document.addEventListener('visibilitychange', () => document.hidden ? pause() : play());
 new IntersectionObserver(([entry]) => {
  visible = entry.isIntersecting;
  if (!visible) return pause();
  if (seen) return play();
  seen = true;
  start = setTimeout(play, 600);
 }, { threshold: 0.15 }).observe(root);
}
