import { LazyMotion, MotionConfig } from 'motion/react';
import type { ReactNode } from 'react';
const features = () => import('../../lib/motion-features').then(m => m.default);
export function IslandRoot({ children }: { children: ReactNode }) {
 return <LazyMotion features={features} strict><MotionConfig reducedMotion="user">{children}</MotionConfig></LazyMotion>;
}
