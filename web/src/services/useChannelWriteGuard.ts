import { useCallback, useLayoutEffect, useRef } from 'react';

/** Retained callbacks must check the current gate, including after their owner unmounts. */
export function useChannelWriteGuard(allowed: boolean) {
  const current = useRef(false);
  useLayoutEffect(() => {
    current.current = allowed;
    return () => {
      current.current = false;
    };
  }, [allowed]);
  return useCallback(() => current.current, []);
}
