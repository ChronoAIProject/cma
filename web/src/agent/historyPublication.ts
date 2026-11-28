import { useCallback, useEffect, type Dispatch, type RefObject, type SetStateAction } from 'react';
import type { AgentResourceState } from './controllerTypes';
import { loadAgent, transientReadFailure, type ReadyAgent } from './loadAgent';
import type { CmaWebClient } from '../api/client';
import { observeRead } from './transport/readObserver';
import { rateLimitDelay } from './transport/readRetry';
import { errorMessage } from './controllerError';
import type { ConversationHydration } from './transport/hydration';

/** History publications reconcile hydration with events already received on the live stream. */
export function useHistoryPublication(
  agentId: string | undefined,
  setState: Dispatch<SetStateAction<AgentResourceState>>,
  hydration: RefObject<ConversationHydration>,
  liveCursor: RefObject<number>,
) {
  return useCallback(
    (ready: ReadyAgent, preserveEmpty = false, retainMetadata = false) => {
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
        return {
          status: 'ready',
          ...(retainMetadata ? retainCurrentMetadata(ready, current) : ready),
          conversation,
        };
      });
    },
    [agentId, setState, hydration, liveCursor],
  );
}

/** Late history pages carry the opening snapshot, not subsequent resource updates. */
export function retainCurrentMetadata(ready: ReadyAgent, current: AgentResourceState): ReadyAgent {
  return current.status === 'ready' &&
    current.v2 === ready.v2 &&
    current.detail.id === ready.detail.id &&
    current.workspace.id === ready.workspace.id &&
    current.detail.sandbox.id === ready.detail.sandbox.id
    ? {
        ...ready,
        detail: current.detail,
        workspace: current.workspace,
        ...(current.workspaceUnavailable ? { workspaceUnavailable: true } : {}),
      }
    : ready;
}

/** Retrying unavailable history is independent of resource-triggered metadata refreshes. */
export function useHistoryRetry({
  agentId,
  client,
  suspended,
  historyUnavailable,
  publishHistory,
  setState,
  stateRef,
  actionAbortRef,
  replayAbortRef,
  hydration,
  metadataVersion,
}: {
  readonly agentId?: string;
  readonly client: CmaWebClient;
  readonly suspended: boolean;
  readonly historyUnavailable: boolean;
  readonly publishHistory: ReturnType<typeof useHistoryPublication>;
  readonly setState: Dispatch<SetStateAction<AgentResourceState>>;
  readonly stateRef: RefObject<AgentResourceState>;
  readonly actionAbortRef: RefObject<AbortController | undefined>;
  readonly replayAbortRef: RefObject<AbortController | undefined>;
  readonly hydration: RefObject<ConversationHydration>;
  readonly metadataVersion: RefObject<number>;
}) {
  useEffect(() => {
    if (suspended || !historyUnavailable || agentId === undefined) return;
    let failures = 0;
    let fallback = 5000;
    const observer = observeRead(
      async (signal) => {
        const metadataAtStart = metadataVersion.current;
        try {
          const ready = await loadAgent(client, agentId, signal);
          if (!signal.aborted)
            publishHistory(ready, false, metadataVersion.current !== metadataAtStart);
        } catch (error) {
          if (signal.aborted) return;
          if (transientReadFailure(error)) {
            fallback = Math.min(fallback * 2, 30000);
            const next = rateLimitDelay(error, fallback, ++failures);
            return { next, cooldown: next, failed: true };
          }
          actionAbortRef.current?.abort();
          replayAbortRef.current?.abort();
          hydration.current.reset();
          setState({ status: 'error', message: errorMessage(error) });
        }
      },
      {
        initialDelay:
          stateRef.current.status === 'ready' && stateRef.current.historyRetryAt
            ? Math.max(1, stateRef.current.historyRetryAt - Date.now())
            : 5000,
      },
    );
    return () => observer.dispose();
  }, [
    agentId,
    client,
    historyUnavailable,
    publishHistory,
    setState,
    stateRef,
    suspended,
    metadataVersion,
    actionAbortRef,
    replayAbortRef,
    hydration,
  ]);
}
