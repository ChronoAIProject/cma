import { useLayoutEffect, useMemo, useRef, useState, type RefObject } from 'react';
import type { AgentVisualAction } from './agentVisual';

export interface MascotPlayback {
  /** One stable object for the current client, viewer, auth epoch, Agent, incarnation and turn. */
  readonly scope: object;
  readonly enabled?: boolean;
}

export function useMascotFrame(
  slot: RefObject<HTMLSpanElement | null>,
  action: AgentVisualAction,
  frameDurations: readonly number[],
  playback?: MascotPlayback,
) {
  const scope = playback?.scope;
  const enabled = playback?.enabled === true;
  const run = useMemo(() => ({ scope, action, enabled }), [scope, action, enabled]);
  const [sample, setSample] = useState<{ run: typeof run; frame?: number }>();
  const intersection = useRef(false);
  const update = useRef<() => void>(undefined);
  const turn = useRef<{ scope: object | undefined; observedWork: boolean; celebrated: boolean }>(
    undefined,
  );

  useLayoutEffect(() => {
    const node = slot.current;
    if (!enabled || !node || typeof IntersectionObserver === 'undefined') return;
    let observing = true;
    const observer = new IntersectionObserver((entries) => {
      if (!observing) return;
      intersection.current = entries.at(-1)?.isIntersecting === true;
      update.current?.();
    });
    observer.observe(node);
    const attributes = new MutationObserver(() => update.current?.());
    for (let element: HTMLElement | null = node; element; element = element.parentElement)
      attributes.observe(element, { attributeFilter: ['hidden', 'inert'] });
    return () => {
      observing = false;
      intersection.current = false;
      observer.disconnect();
      attributes.disconnect();
    };
  }, [enabled, slot]);

  useLayoutEffect(() => {
    const { action, enabled, scope } = run;
    const previous = turn.current;
    const sameTurn = previous !== undefined && previous.scope === scope;
    const love = action === 'love';
    const celebrate = love && sameTurn && previous.observedWork && !previous.celebrated;
    turn.current = {
      scope,
      observedWork:
        (sameTurn && previous.observedWork) ||
        action === 'working' ||
        action === 'idea' ||
        action === 'question',
      celebrated: love || (sameTurn && previous.celebrated),
    };
    if (!enabled || !scope || typeof window.matchMedia !== 'function') return;
    const preference = window.matchMedia('(prefers-reduced-motion: reduce)');
    const visible = () =>
      intersection.current &&
      !document.hidden &&
      !preference.matches &&
      !slot.current?.closest('[hidden], [inert]');
    if (love && (!celebrate || !visible())) return;

    const cycle = frameDurations.reduce((total, duration) => total + duration, 0);
    const limit = Math.min(4000, cycle * (love || action === 'failed' ? 1 : 2));
    let elapsed = 0;
    let started: number | undefined;
    let timer: number | undefined;
    let disposed = false;
    const sync = () => {
      if (disposed) return;
      window.clearTimeout(timer);
      const now = performance.now();
      const time = elapsed + (started === undefined ? 0 : now - started);
      if (!visible() || time >= limit) {
        elapsed = love ? limit : time;
        started = undefined;
        setSample({ run });
        return;
      }
      started ??= now;
      let boundary = 0;
      const frame = frameDurations.findIndex((duration) => {
        boundary += duration;
        return time % cycle < boundary;
      });
      setSample({ run, frame });
      timer = window.setTimeout(sync, Math.ceil(Math.min(limit - time, boundary - (time % cycle))));
    };
    update.current = sync;
    preference.addEventListener('change', sync);
    document.addEventListener('visibilitychange', sync);
    if (visible()) timer = window.setTimeout(sync, 0);
    return () => {
      disposed = true;
      window.clearTimeout(timer);
      update.current = undefined;
      preference.removeEventListener('change', sync);
      document.removeEventListener('visibilitychange', sync);
    };
  }, [run, slot, frameDurations]);

  return sample?.run === run ? sample.frame : undefined;
}
