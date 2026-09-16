import { useCallback, useRef, useState, type SetStateAction } from 'react';
import type { AgentResourceState } from './controller';

/** Apply a frame before acknowledging it, independently of React render scheduling. */
export function useAgentState(initial: AgentResourceState) {
  const [state, render] = useState(initial);
  const current = useRef(state);
  const setState = useCallback((update: SetStateAction<AgentResourceState>) => {
    const next = typeof update === 'function' ? update(current.current) : update;
    current.current = next;
    render(next);
  }, []);
  return [state, setState, current] as const;
}
