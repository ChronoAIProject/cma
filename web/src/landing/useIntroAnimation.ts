import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { buildSchedule, frameAt } from './schedule';
import type { IntroFrame } from './schedule';
import { INTRO_LINES } from './script';

/**
 * Drives the introduction animation.
 *
 * Reduced motion is resolved BEFORE the effect that starts the loop, so under
 * that preference the loop is never created at all. A CSS-only override would
 * still run the animation and still withhold the prompt for ~3 seconds, which
 * is the actual harm — the delay, not the movement.
 */

const REDUCED_MOTION_QUERY = '(prefers-reduced-motion: reduce)';

function subscribeToReducedMotion(onChange: () => void): () => void {
  if (typeof window.matchMedia !== 'function') {
    return () => {};
  }
  const query = window.matchMedia(REDUCED_MOTION_QUERY);
  query.addEventListener('change', onChange);
  return () => {
    query.removeEventListener('change', onChange);
  };
}

function readReducedMotion(): boolean {
  if (typeof window.matchMedia !== 'function') {
    return false;
  }
  return window.matchMedia(REDUCED_MOTION_QUERY).matches;
}

export function usePrefersReducedMotion(): boolean {
  return useSyncExternalStore(subscribeToReducedMotion, readReducedMotion, () => false);
}

const COMPLETE_FRAME: IntroFrame = {
  visibleLines: INTRO_LINES.length,
  charsOnLastLine: Number.MAX_SAFE_INTEGER,
  complete: true,
};

export interface IntroAnimation {
  readonly frame: IntroFrame;
  /** True once the prompt is live, whether by playing through or by skipping. */
  readonly complete: boolean;
  /** Complete the animation immediately. Safe to call repeatedly. */
  readonly skip: () => void;
  readonly reducedMotion: boolean;
}

export function useIntroAnimation(): IntroAnimation {
  const reducedMotion = usePrefersReducedMotion();
  const schedule = useMemo(() => buildSchedule(), []);

  const [rawFrame, setFrame] = useState<IntroFrame>({
    visibleLines: 0,
    charsOnLastLine: 0,
    complete: false,
  });

  // Derived rather than pushed into state from an effect, so a preference that
  // flips mid-animation takes effect on the next render with no cascade.
  const frame = reducedMotion ? COMPLETE_FRAME : rawFrame;

  // Read inside the loop without making it a dependency, so skipping does not
  // tear down and restart the effect.
  const skipped = useRef(false);

  useEffect(() => {
    if (reducedMotion) {
      // The loop is never created at all. A CSS-only override would still run
      // it and still withhold the prompt for seconds, and the delay is the
      // actual harm — not the movement.
      skipped.current = true;
      return;
    }

    let raf = 0;
    let start: number | null = null;

    const tick = (now: number) => {
      if (skipped.current) {
        setFrame(COMPLETE_FRAME);
        return;
      }
      start ??= now;
      const next = frameAt(schedule, now - start);
      setFrame(next);
      if (!next.complete) {
        raf = requestAnimationFrame(tick);
      }
    };

    raf = requestAnimationFrame(tick);

    // React 19 StrictMode runs effects twice in development. Without this the
    // first loop keeps running against a stale setState and the print visibly
    // double-speeds.
    return () => {
      cancelAnimationFrame(raf);
    };
  }, [reducedMotion, schedule]);

  // Stable identity: consumers put this in effect dependency arrays, and an
  // unstable one would retear those effects on every animation frame.
  const skip = useCallback(() => {
    skipped.current = true;
    setFrame(COMPLETE_FRAME);
  }, []);

  return {
    frame,
    complete: frame.complete,
    reducedMotion,
    skip,
  };
}
