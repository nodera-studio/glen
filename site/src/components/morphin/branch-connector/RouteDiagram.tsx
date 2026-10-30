// Adapted from Morphin animated-svg-branch-connector (MIT, see ../LICENSE and site/morphin.json).
// Verbatim source: site/vendor/morphin/animated-svg-branch-connector/components/svg-branches.tsx.
// Two hand-laid layouts (wide and tall) so labels stay at reading size; one in-view trace, no loop.
import { m, useInView, useReducedMotion } from 'motion/react';
import { useId, useRef } from 'react';

type Box = { x: number; y: number; w: number; h: number; title: string; sub?: string };
type Layout = { vb: string; agents: Box[]; glen: Box; sessions: Box[]; routes: string[]; trace: string[]; reply: string; label: { x: number; y: number; anchor: 'start' | 'middle' | 'end' } };
const sessions = (x: number, ys: number[], w: number, h: number): Box[] => [
 { x, y: ys[0]!, w, h, title: 'Live session', sub: 'glen extension' },
 { x, y: ys[1]!, w, h, title: 'Idle session', sub: 'omp RPC' },
 { x, y: ys[2]!, w, h, title: 'New session', sub: '@new' },
];
const wide: Layout = {
 vb: '0 0 640 204',
 agents: [{ x: 8, y: 44, w: 136, h: 40, title: 'Claude Code' }, { x: 8, y: 124, w: 136, h: 40, title: 'Codex' }],
 glen: { x: 236, y: 84, w: 64, h: 40, title: 'glen' },
 sessions: sessions(392, [12, 80, 148], 240, 44),
 routes: ['M144 64H174Q190 64 190 80V92Q190 104 202 104H236', 'M144 144H174Q190 144 190 128V116Q190 104 202 104H236', 'M300 104H330Q346 104 346 88V50Q346 34 362 34H392', 'M300 104H330Q346 104 346 104H392', 'M300 104H330Q346 104 346 120V154Q346 170 362 170H392'],
 trace: ['M144 64H174Q190 64 190 80V92Q190 104 202 104H236', 'M300 104H330Q346 104 346 88V50Q346 34 362 34H392'],
 reply: 'M392 22H92Q76 22 76 30V44',
 label: { x: 236, y: 14, anchor: 'middle' },
};
const tall: Layout = {
 vb: '0 0 360 392',
 agents: [{ x: 8, y: 8, w: 160, h: 40, title: 'Claude Code' }, { x: 184, y: 8, w: 152, h: 40, title: 'Codex' }],
 glen: { x: 140, y: 104, w: 64, h: 40, title: 'glen' },
 sessions: sessions(30, [222, 278, 334], 306, 44),
 routes: ['M88 48V60Q88 76 104 76H156Q172 76 172 92V104', 'M260 48V60Q260 76 244 76H188Q172 76 172 92V104', 'M172 144V164Q172 180 156 180H30Q14 180 14 196V228Q14 244 30 244', 'M14 228V284Q14 300 30 300', 'M14 284V340Q14 356 30 356'],
 trace: ['M88 48V60Q88 76 104 76H156Q172 76 172 92V104', 'M172 144V164Q172 180 156 180H30Q14 180 14 196V228Q14 244 30 244'],
 reply: 'M336 244H340Q352 244 352 232V36Q352 28 344 28H336',
 label: { x: 344, y: 140, anchor: 'end' },
};

function Diagram({ layout, className, visible, reduced }: { layout: Layout; className: string; visible: boolean; reduced: boolean | null }) {
 const id = useId();
 const draw = (i: number) => ({ initial: false as const, animate: { pathLength: reduced || visible ? 1 : 0 }, transition: { duration: reduced ? 0 : .6, delay: reduced ? 0 : i * .5, ease: [.25, 1, .5, 1] as const } });
 const box = (b: Box, strong = false) => <g key={b.title}>
  <rect x={b.x} y={b.y} width={b.w} height={b.h} rx="8" className={strong ? 'fill-ink stroke-ink' : 'fill-content stroke-line-strong'} />
  <text x={b.sub ? b.x + 14 : b.x + b.w / 2} y={b.y + b.h / 2 + 4.5} textAnchor={b.sub ? 'start' : 'middle'} fontSize="13" fontWeight="510" className={strong ? 'fill-content' : 'fill-ink'}>{b.title}</text>
  {b.sub && <text x={b.x + b.w - 14} y={b.y + b.h / 2 + 4} textAnchor="end" fontSize="12" className="fill-ink-subdued" style={{ fontFamily: 'var(--font-mono)' }}>{b.sub}</text>}
 </g>;
 return <svg role="img" aria-labelledby={`${id}-t ${id}-d`} viewBox={layout.vb} className={className}>
  <title id={`${id}-t`}>How glen routes a message</title>
  <desc id={`${id}-d`}>Claude Code or Codex sends through glen to a live session over the glen extension, an idle session over omp RPC, or a new session with @new. The session's glen_reply comes back to the agent that asked.</desc>
  <g fill="none" strokeWidth="1.5" className="stroke-line-strong">{layout.routes.map(d => <path key={d} d={d} />)}</g>
  <path d={layout.reply} fill="none" strokeWidth="1.5" strokeDasharray="3 4" className="stroke-success" opacity=".45" />
  <g fill="none" strokeWidth="2" strokeLinecap="round" aria-hidden="true">
   {layout.trace.map((d, i) => <m.path key={d} d={d} className="stroke-brand" {...draw(i)} />)}
   <m.path d={layout.reply} className="stroke-success" {...draw(2)} />
  </g>
  <text {...layout.label} textAnchor={layout.label.anchor} fontSize="12" className="fill-success" style={{ fontFamily: 'var(--font-mono)' }}>glen_reply</text>
  <g strokeWidth="1">{layout.agents.map(b => box(b))}{box(layout.glen, true)}{layout.sessions.map(b => box(b))}</g>
 </svg>;
}

export function RouteDiagram() {
 const ref = useRef<HTMLDivElement>(null);
 const visible = useInView(ref, { once: true, amount: .4 });
 const reduced = useReducedMotion();
 return <div ref={ref}>
  <Diagram layout={wide} className="route-wide mx-auto w-full max-w-[640px]" visible={visible} reduced={reduced} />
  <Diagram layout={tall} className="route-tall mx-auto w-full max-w-[360px]" visible={visible} reduced={reduced} />
 </div>;
}
