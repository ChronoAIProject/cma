import { useEffect, useRef, useState } from 'react';

/** Animate only the distance to an observed counter, never estimate model usage. */
function ObservedCount({ value }: { readonly value: number }) {
  const [displayed, setDisplayed] = useState(0);
  const current = useRef(0);
  useEffect(() => {
    const motion = window.matchMedia('(prefers-reduced-motion: reduce)');
    const from = Math.min(current.current, value);
    let frame = 0;
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
    frame = requestAnimationFrame(tick);
    motion.addEventListener('change', settle);
    document.addEventListener('visibilitychange', settle);
    return () => {
      cancelAnimationFrame(frame);
      motion.removeEventListener('change', settle);
      document.removeEventListener('visibilitychange', settle);
    };
  }, [value]);
  return <span aria-hidden="true">{Math.min(displayed, value).toLocaleString('en')}</span>;
}

export function TurnUsage({
  usage,
}: {
  readonly usage?: { readonly inputTokens: number; readonly outputTokens: number };
}) {
  return (
    <span className="thinking__usage" aria-live="off">
      {usage === undefined ? (
        'Turn tokens · waiting'
      ) : (
        <>
          <span aria-label={`Turn input tokens: ${usage.inputTokens.toLocaleString('en')}`}>
            <ObservedCount value={usage.inputTokens} /> input
          </span>
          {' · '}
          <span aria-label={`Turn output tokens: ${usage.outputTokens.toLocaleString('en')}`}>
            <ObservedCount value={usage.outputTokens} /> output
          </span>
        </>
      )}
    </span>
  );
}
