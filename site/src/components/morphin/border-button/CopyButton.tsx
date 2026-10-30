// Adapted from Morphin animated-border-button (MIT, see ../LICENSE and site/morphin.json).
// Verbatim source: site/vendor/morphin/animated-border-button/ui/button.tsx and ui/icons/success.tsx.
import { m, AnimatePresence, useReducedMotion } from 'motion/react';
import { useEffect, useRef, useState } from 'react';
function DashedBorderSvg({ solid, animate }: { solid: boolean; animate: boolean }) {
 return <svg aria-hidden="true" className="pointer-events-none absolute inset-0 size-full"><m.rect className="fill-none stroke-current" width="calc(100% - 2px)" height="calc(100% - 2px)" x="1" y="1" rx="6" strokeWidth="1" initial={false} animate={{ strokeDasharray: solid ? '16 0' : '8 8', strokeDashoffset: animate ? -16 : 0 }} transition={{ duration: animate ? .4 : 0, repeat: animate ? 1 : 0, ease: 'linear' }} /></svg>;
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
   await navigator.clipboard.writeText(text); setState('copied');
   timer.current = setTimeout(()=>setState('idle'),2000);
  } catch { setState('error'); }
 }
 return <div className="h-8 min-w-[5.5rem] shrink-0">
  {mounted && <><button type="button" aria-label={`Copy ${label ?? 'command'}`} onClick={copy} className={`focus-ring relative flex h-8 items-center justify-center gap-1 rounded-md border border-line-strong bg-layer-1 px-3 text-ui ${state==='error' ? 'text-danger' : 'text-ink'}`}>
   <DashedBorderSvg solid={state==='copied'} animate={!reduced && state==='copied'} />
   <AnimatePresence initial={false} mode="wait"><m.span key={state} initial={{opacity:0}} animate={{opacity:1}} exit={{opacity:0}} transition={{duration:reduced ? 0 : .12}} className="inline-flex items-center gap-1">
    {state==='copied' && <svg aria-hidden="true" className="size-4 text-success" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2"><circle cx="12" cy="12" r="10" opacity=".4" /><path d="m7 12 3 3 7-7" /></svg>}
    {state==='copied' ? 'Copied' : state==='error' ? 'Copy failed' : 'Copy'}
   </m.span></AnimatePresence>
  </button><span role="status" aria-live="polite" className="sr-only">{state==='copied' ? 'Copied' : state==='error' ? "Couldn't copy. Select the command and copy it yourself." : ''}</span></>}
 </div>;
}
