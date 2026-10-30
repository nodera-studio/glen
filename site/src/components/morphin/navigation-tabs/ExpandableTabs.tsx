// Adapted from Morphin morphing-navigation-tabs-compact-expanded (MIT, see ../LICENSE and site/morphin.json).
// Verbatim source: site/vendor/morphin/morphing-navigation-tabs-compact-expanded/components/ui/expandable-tabs.tsx.
import { m, useReducedMotion } from 'motion/react';
import { useRef, type ReactNode, type KeyboardEvent } from 'react';
interface Props {
 tabs: { id: string; title: string; icon: ReactNode }[];
 selectedId: string; onSelect: (id: string) => void; label: string; idPrefix: string; labels?: 'always';
}
export function ExpandableTabs({ tabs, selectedId, onSelect, label, idPrefix, labels = 'always' }: Props) {
 const reduced = useReducedMotion();
 const refs = useRef<(HTMLButtonElement | null)[]>([]);
 const transition = reduced ? { duration: 0 } : { type: 'spring' as const, stiffness:400, damping:32 };
 function keyDown(event: KeyboardEvent, index: number) {
  const next = event.key === 'ArrowRight' ? (index + 1) % tabs.length : event.key === 'ArrowLeft' ? (index + tabs.length - 1) % tabs.length : event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : null;
  if (next === null) return;
  event.preventDefault(); onSelect(tabs[next]!.id); refs.current[next]?.focus();
 }
 return <div role="tablist" aria-label={label} data-labels={labels} className="tabs-list">
  {tabs.map((tab,index) => {
   const selected = selectedId === tab.id;
   return <m.button type="button" key={tab.id} ref={el=>{ refs.current[index] = el; }} role="tab" id={`${idPrefix}-tab-${tab.id}`} aria-controls={`${idPrefix}-panel-${tab.id}`} aria-selected={selected} aria-label={tab.title} tabIndex={selected ? 0 : -1} onKeyDown={e=>keyDown(e,index)} onClick={()=>onSelect(tab.id)} className={`focus-ring ${selected ? 'text-content' : 'text-ink-subdued'}`}>
    {selected && <m.span className="tab-pill" layoutId={`${idPrefix}-pill`} transition={transition} />}
    <span aria-hidden="true" className="inline-flex size-4 shrink-0">{tab.icon}</span>
    <span>{tab.title}</span>
   </m.button>;
  })}
 </div>;
}
