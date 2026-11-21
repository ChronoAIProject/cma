import { useCallback, type Dispatch, type SetStateAction } from 'react';
import type { AgentResourceState } from './controllerTypes';

/** Rename presentation while retaining the exact draft, stream and conversation. */
export function useAgentTitle(
  agentId: string | undefined,
  setState: Dispatch<SetStateAction<AgentResourceState>>,
) {
  return useCallback(
    (title: string) => {
      setState((current) =>
        current.status === 'ready' && current.detail.id === agentId
          ? {
              ...current,
              detail: { ...current.detail, ownerTitle: title },
              conversation: { ...current.conversation, ownerTitle: title },
            }
          : current,
      );
    },
    [agentId, setState],
  );
}
