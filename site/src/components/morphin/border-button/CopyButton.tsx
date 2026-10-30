// Adapted from Morphin animated-border-button (MIT, see ../LICENSE and site/morphin.json).
// Verbatim source: site/vendor/morphin/animated-border-button/components/ui/button.tsx and components/ui/icons/success.tsx.
// A solid hairline at rest; on copy the border marches as 8 8 dashes, then settles solid in success.
import { m, AnimatePresence, useReducedMotion } from 'motion/react';
import { useEffect, useRef, useState } from 'react';
import { flushSync } from 'react-dom';
function Border({ state, animate }: { state: 'idle' | 'copied' | 'error'; animate: boolean }) {
 return <svg aria-hidden="true" className="pointer-events-none absolute inset-0 size-full overflow-visible"><m.rect className={`fill-none ${state === 'copied' ? 'stroke-success' : state === 'error' ? 'stroke-danger' : 'stroke-line-strong'}`} width="calc(100% - 1px)" height="calc(100% - 1px)" x=".5" y=".5" rx="8" strokeWidth="1" initial={false} animate={{ strokeDasharray: animate ? ['8 8', '8 8', '16 0'] : '16 0', strokeDashoffset: animate ? [0, -32, -48] : 0 }} transition={{ duration: animate ? .6 : 0, ease: [.25, 1, .5, 1] }} /></svg>;
}
export function CopyButton({ text, label }: { text: string; label?: string }) {
 const [mounted,setMounted] = useState(false);
 const [state,setState] = useState<'idle'|'copied'|'error'>('idle');
 const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
 const reduced = useReducedMotion();
 useEffect(()=>{ setMounted(true); return ()=>clearTimeout(timer.current); },[]);
 async function copy() {
  clearTimeout(timer.current);
  try {
   if (!navigator.clipboard) throw new Error('Clipboard unavailable');
   await navigator.clipboard.writeText(text);
   flushSync(()=>setState('idle'));
   queueMicrotask(()=>{
    setState('copied');
    timer.current = setTimeout(()=>setState('idle'),2000);
   });
  } catch { setState('error'); }
 }
 const visibleLabel = state==='copied' ? 'Copied' : state==='error' ? 'Copy failed' : 'Copy';
 return <div className="h-8 min-w-[5.5rem] shrink-0">
  {mounted && <><button type="button" aria-label={`${visibleLabel} ${label ?? 'command'}`} onClick={copy} className={`focus-ring relative flex h-8 w-full items-center justify-center gap-1.5 rounded-lg bg-layer-1 px-3 text-ui-sm font-medium transition-colors hover:bg-layer-2 ${state==='error' ? 'text-danger' : state==='copied' ? 'text-success' : 'text-ink'}`}>
   <Border state={state} animate={!reduced && state==='copied'} />
   <AnimatePresence initial={false} mode="wait"><m.span key={state} initial={{opacity:0,y:4}} animate={{opacity:1,y:0}} exit={{opacity:0,y:-4}} transition={{duration:reduced ? 0 : .14}} className="inline-flex items-center gap-1.5">
    {state==='copied' ? <svg aria-hidden="true" className="size-3.5" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"><path d="m3.5 8.5 3 3 6-7" /></svg>
     : <svg aria-hidden="true" className="size-3.5" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinejoin="round"><rect x="5.5" y="5.5" width="8" height="8" rx="1.5" /><path d="M10.5 3.5v-.5a1 1 0 0 0-1-1h-6a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h.5" /></svg>}
    {visibleLabel}
   </m.span></AnimatePresence>
  </button><span role="status" aria-live="polite" className="sr-only">{state==='copied' ? 'Copied' : state==='error' ? "Couldn't copy. Select the command and copy it yourself." : ''}</span></>}
 </div>;
}
