// Adapted from Morphin morphing-navigation-tabs-compact-expanded (MIT, see ../LICENSE and site/morphin.json).
// Verbatim source: site/vendor/morphin/morphing-navigation-tabs-compact-expanded/expandable-tabs.tsx.
import { AnimatePresence, m, useReducedMotion } from 'motion/react';
import { useRef, type ReactNode, type KeyboardEvent } from 'react';
interface Props {
 tabs: { id: string; title: string; icon: ReactNode }[];
 selectedId: string; onSelect: (id: string) => void; label: string; idPrefix: string;
}
export function ExpandableTabs({ tabs, selectedId, onSelect, label, idPrefix }: Props) {
 const reduced = useReducedMotion();
 const refs = useRef<(HTMLButtonElement | null)[]>([]);
 const transition = reduced ? { duration: 0 } : { type: 'spring' as const, bounce: 0, duration: .6 };
 function keyDown(event: KeyboardEvent, index: number) {
  const next = event.key === 'ArrowRight' ? (index + 1) % tabs.length : event.key === 'ArrowLeft' ? (index + tabs.length - 1) % tabs.length : event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : null;
  if (next === null) return;
  event.preventDefault(); onSelect(tabs[next]!.id); refs.current[next]?.focus();
 }
 return <div role="tablist" aria-label={label} className="mb-4 flex flex-wrap gap-1">
  {tabs.map((tab,index) => {
   const selected = selectedId === tab.id;
   return <m.button type="button" key={tab.id} ref={el=>{ refs.current[index] = el; }} role="tab" id={`${idPrefix}-tab-${tab.id}`} aria-controls={`${idPrefix}-panel-${tab.id}`} aria-selected={selected} aria-label={tab.title} tabIndex={selected ? 0 : -1} onKeyDown={e=>keyDown(e,index)} onClick={()=>onSelect(tab.id)} initial={false} animate={{ paddingLeft: selected ? 16 : 10, paddingRight: selected ? 16 : 10, gap: selected ? 8 : 0 }} transition={transition} className={`focus-ring flex h-10 items-center rounded-md text-ui ${selected ? 'bg-layer-2 text-ink-heading' : 'text-ink-subdued hover:bg-layer-1'}`}>
    <span aria-hidden="true" className="inline-flex size-5 shrink-0">{tab.icon}</span>
    <AnimatePresence initial={false}>{selected && <m.span key="label" className="overflow-hidden whitespace-nowrap" initial={{width:0,opacity:0}} animate={{width:'auto',opacity:1}} exit={{width:0,opacity:0}} transition={transition}>{tab.title}</m.span>}</AnimatePresence>
   </m.button>;
  })}
 </div>;
}
