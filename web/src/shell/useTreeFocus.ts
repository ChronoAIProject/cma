import { useLayoutEffect, useRef, type FocusEvent } from 'react';

export function useTreeFocus() {
  const ref = useRef<HTMLDivElement>(null);
  const focused = useRef<{ element: HTMLElement; node: string } | undefined>(undefined);
  useLayoutEffect(() => {
    const previous = focused.current;
    if (!previous || previous.element.isConnected) return;
    focused.current = undefined;
    if (document.activeElement !== document.body) return;
    const row = Array.from(ref.current?.querySelectorAll<HTMLElement>('[data-node-id]') ?? []).find(
      (candidate) => candidate.dataset.nodeId === previous.node,
    );
    if (row) row.focus();
  });
  return {
    ref,
    onFocusCapture: (event: FocusEvent<HTMLDivElement>) => {
      const node = event.target.closest<HTMLElement>('[data-node-id]')?.dataset.nodeId;
      focused.current = node ? { element: event.target, node } : undefined;
    },
    onBlurCapture: (event: FocusEvent<HTMLDivElement>) => {
      if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget))
        focused.current = undefined;
    },
  };
}
