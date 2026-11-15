import { useCallback, type Dispatch, type RefObject, type SetStateAction } from 'react';
import type { AgentResourceState } from './controllerTypes';
import type { ReadyAgent } from './loadAgent';
import type { ConversationHydration } from './transport/hydration';

/** History publications reconcile hydration with events already received on the live stream. */
export function useHistoryPublication(
  agentId: string | undefined,
  setState: Dispatch<SetStateAction<AgentResourceState>>,
  hydration: RefObject<ConversationHydration>,
  liveCursor: RefObject<number>,
) {
  return useCallback(
    (ready: ReadyAgent, preserveEmpty = false) => {
      if (agentId === undefined) return;
      setState((current) => {
        const conversation =
          preserveEmpty &&
          current.status === 'ready' &&
          ready.conversation.agentCursor === undefined &&
          ready.conversation.cursor === 0 &&
          ready.conversation.messages.length === 0
            ? current.conversation
            : hydration.current.merge(
                ready.conversation,
                agentId,
                !ready.historyLoading && !ready.historyUnavailable,
              );
        liveCursor.current = conversation.cursor;
        return { status: 'ready', ...ready, conversation };
      });
    },
    [agentId, setState, hydration, liveCursor],
  );
}
