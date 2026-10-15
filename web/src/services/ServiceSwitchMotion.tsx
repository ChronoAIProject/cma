import {
  createContext,
  useContext,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from 'react';

type ServiceSelection = 'agent' | 'gateway';
const MotionContext = createContext<{
  current: ServiceSelection;
  previous: ServiceSelection;
  revision: number;
} | null>(null);

/** The switch crosses retained portal frames, so its prior position belongs to the shell. */
export function ServiceSwitchMotion({
  current,
  children,
}: {
  readonly current: ServiceSelection;
  readonly children: ReactNode;
}) {
  const [selection, setSelection] = useState({ current, previous: current, revision: 0 });
  if (selection.current !== current)
    setSelection({ current, previous: selection.current, revision: selection.revision + 1 });
  return <MotionContext value={selection}>{children}</MotionContext>;
}

export function useServiceIndicator(collapsed: boolean) {
  const selection = useContext(MotionContext);
  const ref = useRef<HTMLSpanElement>(null);
  const last = useRef<{ revision: number; collapsed: boolean } | null>(null);
  useLayoutEffect(() => {
    if (!selection) return;
    const previous = last.current;
    last.current = { revision: selection.revision, collapsed };
    if (previous?.revision === selection.revision && previous.collapsed !== collapsed) return;
    const element = ref.current;
    if (
      !element ||
      selection.previous === selection.current ||
      element.closest('[hidden]') ||
      typeof element.animate !== 'function'
    )
      return;
    const preference = window.matchMedia('(prefers-reduced-motion: reduce)');
    if (preference.matches) return;
    const style = getComputedStyle(element);
    const token = style.getPropertyValue('--ds-transition-duration-slow').trim();
    const parsed = Number.parseFloat(token);
    const duration = Number.isFinite(parsed) ? parsed * (token.endsWith('ms') ? 1 : 1000) : 300;
    const transform = (value: ServiceSelection) =>
      `translate${collapsed ? 'Y' : 'X'}(${value === 'gateway' ? 100 : 0}%)`;
    const animation = element.animate(
      [{ transform: transform(selection.previous) }, { transform: transform(selection.current) }],
      {
        duration,
        easing: style.getPropertyValue('--ds-ease-in-out').trim() || 'cubic-bezier(0.4, 0, 0.2, 1)',
      },
    );
    const changed = () => {
      if (preference.matches) animation.cancel();
    };
    preference.addEventListener('change', changed);
    return () => {
      animation.cancel();
      preference.removeEventListener('change', changed);
    };
  }, [selection, collapsed]);
  return ref;
}
