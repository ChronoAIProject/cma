import {
  cloneElement,
  createContext,
  useContext,
  useEffect,
  useState,
  useSyncExternalStore,
  type HTMLAttributes,
  type ReactElement,
} from 'react';
import './Presence.css';

const motionQuery = '(prefers-reduced-motion: reduce)';
const ExitingContext = createContext(false);

/** A conditionally mounted dialog must also release its portalled contents. */
export const usePresenceExiting = () => useContext(ExitingContext);
const reducedMotion = () => window.matchMedia(motionQuery).matches;
const subscribeMotion = (notify: () => void) => {
  const query = window.matchMedia(motionQuery);
  query.addEventListener('change', notify);
  return () => query.removeEventListener('change', notify);
};

/** Keep only an inert exit frame; a new opening cancels its removal timer. */
export function usePresence(open: boolean) {
  const reduced = useSyncExternalStore(subscribeMotion, reducedMotion, () => false);
  const [retained, setRetained] = useState(open);
  const [previousOpen, setPreviousOpen] = useState(open);
  if (open !== previousOpen) {
    setPreviousOpen(open);
    if (open) setRetained(true);
  }
  useEffect(() => {
    if (open || !retained) return;
    const timer = window.setTimeout(() => setRetained(false), reduced ? 0 : 200);
    return () => window.clearTimeout(timer);
  }, [open, retained, reduced]);
  return { present: open || (!reduced && retained), exiting: !open };
}

type NoticeElement = ReactElement<HTMLAttributes<HTMLElement>>;

/** Animate a conditional message without introducing a layout wrapper. */
export function Presence({ children }: { readonly children: NoticeElement | null | false }) {
  const parentExiting = usePresenceExiting();
  const current = parentExiting ? null : children || null;
  const { present, exiting } = usePresence(current !== null);
  const [last, setLast] = useState<NoticeElement | null>(current);
  if (current !== null && current !== last) setLast(current);
  if (!present && last !== null) setLast(null);
  const element = current ?? last;
  if (!present || element === null) return null;
  return (
    <ExitingContext value={exiting}>
      {cloneElement(element, {
        ...presenceAttributes(exiting),
        'aria-hidden': exiting ? true : element.props['aria-hidden'],
        inert: exiting || element.props.inert,
      })}
    </ExitingContext>
  );
}

export function presenceAttributes(exiting: boolean) {
  return {
    'data-presence': exiting ? ('exiting' as const) : ('visible' as const),
    'aria-hidden': exiting || undefined,
    inert: exiting || undefined,
  };
}
