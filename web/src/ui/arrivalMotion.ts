import { useLayoutEffect, useRef } from 'react';

/** Animate an existing surface; identity, focus and layout remain owned by its caller. */
export function animateArrival(element: HTMLElement, pace: 'normal' | 'slow' = 'normal') {
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
  const animation = element.animate([{ opacity: 0 }, { opacity: 1 }], {
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
  useLayoutEffect(() => {
    const changed = previous.current.identity !== identity || previous.current.visible !== visible;
    previous.current = { identity, visible };
    if (!visible || (!changed && !enterOnMount) || ref.current === null) return;
    const animation = animateArrival(ref.current, 'slow');
    return () => animation?.cancel();
  }, [identity, visible, enterOnMount]);
  return ref;
}
