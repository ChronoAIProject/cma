import { useLayoutEffect, type RefObject } from 'react';

function fit(field: HTMLTextAreaElement): void {
  field.style.height = 'auto';
  field.style.height = `${String(field.scrollHeight)}px`;
}

export function useComposerResize(
  inputRef: RefObject<HTMLTextAreaElement | null>,
  visibleContent: string,
): void {
  useLayoutEffect(() => {
    const field = inputRef.current;
    if (field !== null) fit(field);
  }, [inputRef, visibleContent]);

  useLayoutEffect(() => {
    const field = inputRef.current;
    if (field === null) return;
    let width = field.clientWidth;
    let frame: number | undefined;
    const observer = new ResizeObserver(() => {
      if (field.clientWidth === width) return;
      width = field.clientWidth;
      if (frame !== undefined) return;
      frame = requestAnimationFrame(() => {
        frame = undefined;
        fit(field);
      });
    });
    observer.observe(field);
    return () => {
      observer.disconnect();
      if (frame !== undefined) cancelAnimationFrame(frame);
    };
  }, [inputRef]);
}
