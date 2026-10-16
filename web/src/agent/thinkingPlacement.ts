import { useLayoutEffect, useRef } from 'react';
import type { ConversationState } from './model';

/** Follow measured reply growth without moving messages or changing the reader's scroll. */
export function useThinkingPlacement(conversation: ConversationState) {
  const ref = useRef<HTMLDivElement>(null);
  const previous = useRef<{ element: HTMLElement; top: number; run: string } | undefined>(
    undefined,
  );
  const motion = useRef<Animation | undefined>(undefined);
  useLayoutEffect(() => {
    const element = ref.current;
    const run = conversation.activeRunId;
    if (element === null || run === undefined) {
      previous.current = undefined;
      motion.current?.cancel();
      return;
    }
    const before = previous.current;
    const top = element.offsetTop;
    previous.current = { element, top, run };
    if (before?.element !== element || before.run !== run || before.top === top) return;
    const preference = window.matchMedia('(prefers-reduced-motion: reduce)');
    if (
      preference.matches ||
      document.hidden ||
      element.closest('[hidden], [inert]') ||
      typeof element.animate !== 'function'
    )
      return;
    const style = getComputedStyle(element);
    const transform = style.transform;
    const shift =
      typeof DOMMatrixReadOnly === 'undefined' || transform === 'none' || transform === ''
        ? 0
        : new DOMMatrixReadOnly(transform).m42;
    motion.current?.cancel();
    const token = style.getPropertyValue('--ds-transition-duration').trim();
    const parsed = Number.parseFloat(token);
    const duration = Number.isFinite(parsed) ? parsed * (token.endsWith('ms') ? 1 : 1000) : 200;
    const animation = element.animate(
      [{ transform: `translateY(${before.top - top + shift}px)` }, { transform: 'translateY(0)' }],
      {
        duration,
        easing: style.getPropertyValue('--ds-ease-in-out').trim() || 'cubic-bezier(0.4, 0, 0.2, 1)',
      },
    );
    motion.current = animation;
    const changed = () => {
      if (preference.matches || document.hidden) animation.cancel();
    };
    const release = () => {
      preference.removeEventListener('change', changed);
      document.removeEventListener('visibilitychange', changed);
      animation.removeEventListener('finish', release);
      animation.removeEventListener('cancel', release);
    };
    preference.addEventListener('change', changed);
    document.addEventListener('visibilitychange', changed);
    animation.addEventListener('finish', release);
    animation.addEventListener('cancel', release);
  }, [conversation]);
  useLayoutEffect(() => () => motion.current?.cancel(), []);
  return ref;
}
