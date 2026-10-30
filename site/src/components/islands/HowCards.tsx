// Adapted from Morphin animated-feature-cards-for-react-tailwind-framer-motion (stages 01, 03, 04)
// and text-shimmer-wave (zDistance 6, scaleDistance 1.06, two cycles). The upstream setTimeout
// loops are gone: each stage plays once, one at a time, when 40% of it is in view. The markup
// arrives as static children already showing the finished stage.
import { useEffect, useRef, type ReactNode } from 'react';

const OUT = 'cubic-bezier(.25,1,.5,1)';
const SPRING = 'cubic-bezier(.34,1.56,.64,1)';

async function play(stage: HTMLElement): Promise<unknown> {
 const all = (els: Iterable<Element>, frames: Keyframe[], opts: (i: number) => KeyframeAnimationOptions) =>
  Promise.all(Array.from(els, (el, i) => el.animate(frames, { easing: OUT, fill: 'backwards', ...opts(i) }).finished));
 const kind = stage.dataset.stage;
 if (kind === 'find') return all(stage.querySelectorAll('[data-row]'), [{ opacity: 0, transform: 'translateY(8px)' }, { opacity: 1, transform: 'none' }], i => ({ duration: 400, delay: i * 80 }));
 if (kind === 'read') return all(stage.querySelectorAll('[data-row]'), [{ opacity: 0, transform: 'translateY(4px)' }, { opacity: 1, transform: 'none' }], i => ({ duration: 300, delay: i * 90 }));
 if (kind === 'send') {
  const chip = stage.querySelector<HTMLElement>('[data-chip]')!, from = stage.querySelector('.source')!.getBoundingClientRect(), to = chip.getBoundingClientRect();
  const dx = from.left + 12 - to.left, dy = from.bottom + 8 - to.top;
  return chip.animate([{ opacity: 0, transform: `translate(${dx}px, ${dy}px) scale(.92)` }, { opacity: 1, offset: .2 }, { opacity: 1, transform: 'none' }], { duration: 640, easing: SPRING, fill: 'backwards' }).finished
   .then(() => stage.querySelector('[data-target]')!.animate([{ boxShadow: '0 0 0 0 color-mix(in oklch, var(--brand) 40%, transparent)' }, { boxShadow: '0 0 0 8px transparent' }], { duration: 500, easing: OUT }).finished);
 }
 const chars = stage.querySelectorAll('.shimmer > span');
 return all(chars, [
  { transform: 'translateZ(0) scale(1)', color: 'var(--base-color)' },
  { transform: 'translateZ(6px) scale(1.06)', color: 'var(--base-gradient-color)' },
  { transform: 'translateZ(0) scale(1)', color: 'var(--base-color)' },
 ], i => ({ duration: 900, delay: i * 40, iterations: 2, easing: 'ease-in-out' })).then(() => {
  stage.dataset.state = 'done';
  return all(stage.querySelectorAll('[data-after]'), [{ opacity: 0, transform: 'translateY(4px)' }, { opacity: 1, transform: 'none' }], i => ({ duration: 300, delay: i * 70 }));
 });
}

export default function HowCards({ children }: { children?: ReactNode }) {
 const ref = useRef<HTMLDivElement>(null);
 useEffect(() => {
  if (matchMedia('(prefers-reduced-motion: reduce)').matches) return;
  const stages = Array.from(ref.current?.querySelectorAll<HTMLElement>('[data-stage]') ?? []);
  const queue: HTMLElement[] = [];
  let busy = false, gone = false;
  const next = async () => {
   if (busy) return;
   busy = true;
   while (queue.length && !gone) {
    const stage = queue.shift()!;
    if (stage.dataset.stage !== 'hear') stage.dataset.state = 'done';
    await play(stage).catch(() => {});
    stage.dataset.state = 'done';
   }
   busy = false;
  };
  stages.forEach(stage => { stage.dataset.state = 'armed'; });
  const io = new IntersectionObserver(entries => {
   for (const e of entries) if (e.isIntersecting) { io.unobserve(e.target); queue.push(e.target as HTMLElement); }
   void next();
  }, { threshold: .4 });
  stages.forEach(stage => io.observe(stage));
  return () => { gone = true; io.disconnect(); stages.forEach(stage => { stage.dataset.state = 'done'; }); };
 }, []);
 return <div ref={ref}>{children}</div>;
}
