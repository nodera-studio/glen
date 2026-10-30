// Adapted from Morphin animated-feature-cards-for-react-tailwind-framer-motion.
// Stages 01/04/03: finite, serialized in-view playback; SSR is the finished stage.
// Adapted from Morphin text-shimmer-wave: zDistance 6, scaleDistance 1.06, two cycles.
import { useEffect, useRef } from 'react';
import { m } from 'motion/react';
import { IslandRoot } from './IslandRoot';
import palette from '../../../../src/render/palette.json';
import type { CSSProperties } from 'react';
const steps=[
 ['Find','glen ls','Lists the omp sessions for this folder, live or idle, with short handles.'],
 ['Read','glen show / glen tail -f',"Prints a session’s transcript, or follows it as it grows."],
 ['Send','glen send','Messages an open or idle session, or starts a new one with @new.'],
 ['Hear back','glen_reply, then glen inbox','Returns the answer to your agent. Later replies stay in glen inbox.'],
];
const vars=Object.fromEntries(palette.tokens.map(t=>[`--glen-${t.name}`,t.dark.hex])) as CSSProperties;
export default function HowCards() {
 const ref=useRef<HTMLDivElement>(null);
 useEffect(()=>{
  if(matchMedia('(prefers-reduced-motion: reduce)').matches)return;
  const pending:HTMLElement[]=[];const seen=new Set<Element>();const animations:Animation[]=[];let running=false,disposed=false;
  const play=async()=>{
   if(running)return;running=true;
   while(pending.length && !disposed){
    const card=pending.shift()!;
    const run=(el:Element,frames:Keyframe[],options:KeyframeAnimationOptions)=>{const a=el.animate(frames,{easing:'cubic-bezier(.25,1,.5,1)',...options});animations.push(a);return a.finished.catch(()=>{});};
    const items=card.querySelectorAll('[data-stage-row]');
    if(card.dataset.stage==='2') {
     // NX settle spring: duration .4, bounce .1, sampled as a settling path.
     await Promise.all(Array.from(items,el=>run(el,[{transform:'translate(-40px,-35px)',opacity:0},{transform:'translate(2px,1px)',opacity:1,offset:.8},{transform:'translate(0,0)',opacity:1}],{duration:400})));
    } else if(card.dataset.stage==='3') {
     const reply=card.querySelector('.mini-reply')!;const wait=card.querySelector('.shimmer')!;
     await Promise.all([run(reply,[{opacity:0},{opacity:0}],{duration:2000}),run(wait,[{opacity:1},{opacity:1}],{duration:2000}),...Array.from(wait.children,(el,i)=>run(el,[{transform:'translateZ(0) scale(1)',color:'var(--base-color)'},{transform:'translateZ(6px) scale(1.06)',color:'var(--base-gradient-color)'},{transform:'translateZ(0) scale(1)',color:'var(--base-color)'}],{duration:800,iterations:2,delay:i*24,fill:'backwards'}))]);
    } else await Promise.all(Array.from(items,(el,i)=>run(el,[{transform:'translateY(8px)',opacity:0},{transform:'translateY(0)',opacity:1}],{duration:400,delay:i*80,fill:'backwards'})));
    card.dataset.played='true';
   }
   running=false;
  };
  const observer=new IntersectionObserver(entries=>{for(const e of entries)if(e.isIntersecting && !seen.has(e.target)){seen.add(e.target);pending.push(e.target as HTMLElement);observer.unobserve(e.target);}void play();},{threshold:.4});
  ref.current?.querySelectorAll<HTMLElement>('[data-stage]').forEach(el=>observer.observe(el));
  return()=>{disposed=true;observer.disconnect();animations.forEach(a=>a.cancel());};
 },[]);
 return <IslandRoot><div className="how-grid" ref={ref} style={vars}>{steps.map(([title,command,body],i)=><m.article key={title} className="how-card reveal" data-stage={i}>
  <div className="how-stage"><div className="mini-terminal" aria-label={`${title} example`}>
   {i===0 && <><div className="t-muted">handle   state   title</div>{['e27c0de  ● live  Fix migration 0012','b7e2d81  ○ idle  Review routes','b8e4a07  ○ idle  Add fixtures'].map(text=><div data-stage-row key={text}>{text}</div>)}<div className="t-muted">3 sessions · 1 live</div></>}
   {i===1 && <><div data-stage-row className="t-accent">❯ user · 12:04</div><div data-stage-row className="t-accent_2">◆ assistant · 12:04</div><div data-stage-row>↳ bash cargo test …</div></>}
   {i===2 && <div className="message-route"><span className="mini-you">you</span><span data-stage-row className="message-chip">Fix 0012 and tell me.</span><span className="mini-target">e27c0de <span className="t-success">● live</span></span></div>}
   {i===3 && <div style={{position:'relative'}}><span className="shimmer" style={{position:'absolute',inset:0,opacity:0}} aria-hidden="true">{Array.from('omp is working',(c,j)=><span key={j}>{c}</span>)}</span><span className="mini-reply t-success">↩ All 12 tests pass. ✓</span></div>}
  </div></div><div className="how-caption"><h3 className="text-title-sm">{title}</h3><code>{command}</code><p>{body}</p></div>
 </m.article>)}</div></IslandRoot>;
}
