// Adapted from Morphin morphing-navigation-tabs-compact-expanded (MIT, see ../LICENSE and site/morphin.json).
// Verbatim source: site/vendor/morphin/morphing-navigation-tabs-compact-expanded/components/ui/expandable-tabs.tsx.
// Labels always show. The selected pill glides between tabs with a FLIP animation on the ink pill
// (the NX app's layoutId pill, spring 400/32), which needs no motion layout features.
import { useLayoutEffect, useRef, type ReactNode, type KeyboardEvent } from 'react';
interface Props {
 tabs: { id: string; title: string; icon: ReactNode }[];
 selectedId: string; onSelect: (id: string) => void; label: string; idPrefix: string; labels?: 'always';
}
// A critically damped-ish spring (stiffness 400, damping 32) sampled for linear().
const SPRING = 'linear(0, .23 6%, .56 13%, .79 20%, .92 28%, .98 36%, 1.005 46%, 1.006 58%, 1)';
export function ExpandableTabs({ tabs, selectedId, onSelect, label, idPrefix, labels = 'always' }: Props) {
 const refs = useRef<(HTMLButtonElement | null)[]>([]);
 const pill = useRef<HTMLSpanElement>(null);
 const last = useRef<DOMRect | null>(null);
 useLayoutEffect(() => {
  const el = pill.current;
  if (!el) return;
  const now = el.getBoundingClientRect();
  const prev = last.current;
  last.current = now;
  if (!prev || matchMedia('(prefers-reduced-motion: reduce)').matches) return;
  const dx = prev.left - now.left, sx = prev.width / now.width;
  if (Math.abs(dx) < 1) return;
  let easing = SPRING;
  try { el.animate([{}, {}], { easing }); } catch { easing = 'cubic-bezier(.25,1,.5,1)'; }
  el.animate([{ transform: `translateX(${dx}px) scaleX(${sx})`, transformOrigin: 'left' }, { transform: 'none', transformOrigin: 'left' }], { duration: 420, easing });
 }, [selectedId]);
 function keyDown(event: KeyboardEvent, index: number) {
  const next = event.key === 'ArrowRight' ? (index + 1) % tabs.length : event.key === 'ArrowLeft' ? (index + tabs.length - 1) % tabs.length : event.key === 'Home' ? 0 : event.key === 'End' ? tabs.length - 1 : null;
  if (next === null) return;
  event.preventDefault(); onSelect(tabs[next]!.id); refs.current[next]?.focus();
 }
 return <div role="tablist" aria-label={label} data-labels={labels} className="tabs-list">
  {tabs.map((tab,index) => {
   const selected = selectedId === tab.id;
   return <button type="button" key={tab.id} ref={el=>{ refs.current[index] = el; }} role="tab" id={`${idPrefix}-tab-${tab.id}`} aria-controls={`${idPrefix}-panel-${tab.id}`} aria-selected={selected} tabIndex={selected ? 0 : -1} onKeyDown={e=>keyDown(e,index)} onClick={()=>onSelect(tab.id)} className={`focus-ring ${selected ? 'text-content' : 'text-ink-subdued'}`}>
    {selected && <span ref={pill} className="tab-pill" aria-hidden="true" />}
    <span aria-hidden="true" className="inline-flex size-4 shrink-0">{tab.icon}</span>
    <span>{tab.title}</span>
   </button>;
  })}
 </div>;
}
