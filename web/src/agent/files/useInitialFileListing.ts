import { useCallback, useEffect, useRef, type RefObject } from 'react';
import type { FileNavigationMemory } from './useFileNavigation';

export function useInitialFileListing(
  active: boolean,
  preserveListing: boolean,
  initialRoot: string,
  navigationMemory: FileNavigationMemory | undefined,
  load: (path: string, signal: AbortSignal) => Promise<boolean>,
  requestRef: RefObject<AbortController | undefined>,
) {
  const started = useRef(false);
  const retained = active || preserveListing;
  const list = useCallback(
    (signal: AbortSignal) => {
      void load(initialRoot, signal);
      const remembered = navigationMemory?.read();
      if (remembered !== undefined && remembered !== initialRoot) void load(remembered, signal);
    },
    [initialRoot, load, navigationMemory],
  );
  useEffect(() => {
    if (!retained) return;
    const controller = new AbortController();
    requestRef.current = controller;
    started.current = false;
    return () => controller.abort();
  }, [retained, list, requestRef]);
  useEffect(() => {
    const signal = requestRef.current?.signal;
    if (!active || !signal) return;
    let cancelled = false;
    queueMicrotask(() => {
      if (cancelled || signal.aborted || started.current) return;
      started.current = true;
      list(signal);
    });
    return () => {
      cancelled = true;
    };
  }, [active, retained, list, requestRef]);
}
