import { useLayoutEffect, useRef } from 'react';

/** Animate an existing surface; identity, focus and layout remain owned by its caller. */
export function animateArrival(
  element: HTMLElement,
  pace: 'normal' | 'slow' = 'normal',
  fromOpacity = 0,
) {
  if (typeof element.animate !== 'function' || element.closest('[hidden], [inert]')) return;
  const preference = window.matchMedia('(prefers-reduced-motion: reduce)');
  if (preference.matches) return;
  const style = getComputedStyle(element);
  const token = style
    .getPropertyValue(
      pace === 'slow' ? '--ds-transition-duration-slow' : '--ds-transition-duration',
    )
    .trim();
  const value = Number.parseFloat(token);
  const duration = Number.isFinite(value)
    ? value * (token.endsWith('ms') ? 1 : 1000)
    : pace === 'slow'
      ? 300
      : 200;
  const animation = element.animate([{ opacity: fromOpacity }, { opacity: 1 }], {
    duration,
    easing: style.getPropertyValue('--ds-ease-in-out').trim() || 'cubic-bezier(0.4, 0, 0.2, 1)',
  });
  const changed = () => {
    if (preference.matches) animation.cancel();
  };
  const release = () => {
    preference.removeEventListener('change', changed);
    animation.removeEventListener('finish', release);
    animation.removeEventListener('cancel', release);
  };
  preference.addEventListener('change', changed);
  animation.addEventListener('finish', release);
  animation.addEventListener('cancel', release);
  return animation;
}

/** Route changes and re-entry fade the same node, without remounting its working state. */
export function useViewArrival<Element extends HTMLElement = HTMLElement>(
  identity: string,
  visible = true,
  enterOnMount = true,
) {
  const ref = useRef<Element>(null);
  const previous = useRef({ identity, visible });
  const interrupted = useRef<{ element: Element; opacity: number } | undefined>(undefined);
  useLayoutEffect(() => {
    const changed = previous.current.identity !== identity || previous.current.visible !== visible;
    previous.current = { identity, visible };
    const continuation = interrupted.current;
    interrupted.current = undefined;
    if (!visible || (!changed && !enterOnMount) || ref.current === null) return;
    const element = ref.current;
    const animation = animateArrival(
      element,
      'slow',
      continuation?.element === element ? continuation.opacity : 0,
    );
    return () => {
      // Read the presented opacity before cancel reveals the underlying style. A fast
      // second navigation continues this surface's fade instead of flashing it blank.
      if (animation?.playState === 'running' || animation?.playState === 'paused') {
        const opacity = Number.parseFloat(getComputedStyle(element).opacity);
        if (Number.isFinite(opacity)) interrupted.current = { element, opacity };
      }
      animation?.cancel();
    };
  }, [identity, visible, enterOnMount]);
  return ref;
}
