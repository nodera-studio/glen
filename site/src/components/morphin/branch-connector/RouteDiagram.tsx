// Adapted from Morphin animated-svg-branch-connector (MIT, see ../LICENSE and site/morphin.json).
// Verbatim source: site/vendor/morphin/animated-svg-branch-connector/components/svg-branches.tsx.
import { m, useInView, useReducedMotion } from 'motion/react';
import { useId, useRef } from 'react';
const paths = [
 'M104 72H112Q136 72 136 96V112H167',
 'M104 152H112Q136 152 136 128V112H167',
 'M210 112H227V60Q227 50 240 50H260',
 'M210 112H260',
 'M210 112H227V164Q227 174 240 174H260',
];
const reply = 'M306 30V12H42Q20 12 20 34V50M20 34V130';
export function RouteDiagram() {
 const ref = useRef<SVGSVGElement>(null);
 const visible = useInView(ref,{once:true,amount:.4});
 const reduced = useReducedMotion();
 const id = useId();
 return <svg ref={ref} role="img" aria-labelledby={`${id}-title ${id}-desc`} viewBox="0 0 380 220" className="mx-auto block w-full max-w-[520px]">
  <title id={`${id}-title`}>How glen routes messages</title>
  <desc id={`${id}-desc`}>Claude Code and Codex send through glen to a live session using the glen extension, an idle session using omp RPC, or a new session with @new. The live session returns glen_reply to the calling agent.</desc>
  <g fill="none" strokeWidth="1.5" className="stroke-line-strong">{paths.map(d=><path key={d} d={d} />)}<path d={reply} strokeDasharray="4 4" /></g>
  <g fill="none" strokeWidth="2" aria-hidden="true">
   {[paths[0],paths[1],paths[2],reply].map((d,i)=><m.path key={d} d={d} className={i===3 ? 'stroke-success' : 'stroke-brand'} initial={false} animate={{pathLength:reduced || visible ? 1 : 0}} transition={{duration:reduced ? 0 : .65,delay:0}} />)}
  </g>
  <g className="fill-layer-1 stroke-line-strong"><rect x="4" y="52" width="100" height="36" rx="6"/><rect x="4" y="132" width="100" height="36" rx="6"/><rect x="167" y="94" width="43" height="36" rx="6"/></g>
  <g className="fill-ink" fontSize="14" fontWeight="500" textAnchor="middle"><text x="54" y="75">Claude Code</text><text x="54" y="155">Codex</text><text x="188" y="117">glen</text></g>
  <g className="fill-ink" fontSize="14" fontWeight="500"><text x="264" y="48">Live session</text><text x="264" y="110">Idle session</text><text x="264" y="172">New session</text></g>
  <g className="fill-ink-subdued" fontSize="13.5"><text x="264" y="67">glen extension</text><text x="264" y="129">omp RPC</text><text x="264" y="191">@new</text><text x="121" y="29">glen_reply</text></g>
 </svg>;
}
