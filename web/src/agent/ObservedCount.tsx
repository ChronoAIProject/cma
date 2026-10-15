import { useEffect, useRef, useState } from 'react';

/** Animate only the distance to an observed counter, never estimate model usage. */
export function ObservedCount({
  value,
  animateInitial = true,
}: {
  readonly value: number;
  readonly animateInitial?: boolean;
}) {
  const [displayed, setDisplayed] = useState(animateInitial ? 0 : value);
  const current = useRef(displayed);
  useEffect(() => {
    const motion = window.matchMedia('(prefers-reduced-motion: reduce)');
    const from = Math.min(current.current, value);
    let frame = 0;
    let cancelled = false;
    let started: number | undefined;
    const publish = (next: number) => {
      current.current = next;
      setDisplayed(next);
    };
    const tick = (now: number) => {
      started ??= now;
      const progress = motion.matches || document.hidden ? 1 : Math.min(1, (now - started) / 600);
      publish(Math.min(value, Math.floor(from + (value - from) * (1 - (1 - progress) ** 3))));
      if (progress < 1) frame = requestAnimationFrame(tick);
    };
    const settle = () => {
      if (!motion.matches && !document.hidden) return;
      cancelAnimationFrame(frame);
      publish(value);
    };
    // Background documents may never receive an animation frame. Keep the
    // actual observation available immediately when animation is suppressed.
    if (motion.matches || document.hidden)
      queueMicrotask(() => {
        if (!cancelled) publish(value);
      });
    else frame = requestAnimationFrame(tick);
    motion.addEventListener('change', settle);
    document.addEventListener('visibilitychange', settle);
    return () => {
      cancelled = true;
      cancelAnimationFrame(frame);
      motion.removeEventListener('change', settle);
      document.removeEventListener('visibilitychange', settle);
    };
  }, [value]);
  return <span aria-hidden="true">{Math.min(displayed, value).toLocaleString('en')}</span>;
}
