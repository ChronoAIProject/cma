import { useEffect, type Dispatch, type RefObject, type SetStateAction } from 'react';
import type { CmaWebClient } from '../api/client';
import type { SandboxEvent } from '../api/types';
import { readSandboxHistory } from '../sandbox/eventHistory';
import type { AgentCommand, AgentResourceState } from './controllerTypes';
import { observeRead } from './transport/readObserver';
import { retrySchedule } from './transport/readRetry';
import { readPreparation } from './preparationRead';
import { refreshConversation } from './reopenAgent';
import type { ReadinessWake } from './readinessWait';

interface Options {
  agentId?: string;
  client: CmaWebClient;
  pending?: AgentCommand;
  provisioning: boolean;
  stateRef: RefObject<AgentResourceState>;
  setState: Dispatch<SetStateAction<AgentResourceState>>;
  setActionError: Dispatch<SetStateAction<string | undefined>>;
  liveCursor: RefObject<number>;
  hierarchyChanged: RefObject<(() => void) | undefined>;
  onIdentityChange: (error: unknown) => boolean;
  wake: ReadinessWake;
}

export function usePreparationProgress({
  agentId,
  client,
  pending,
  provisioning,
  stateRef,
  setState,
  setActionError,
  liveCursor,
  hierarchyChanged,
  onIdentityChange,
  wake,
}: Options) {
  useEffect(() => {
    if (!provisioning || agentId === undefined || pending !== undefined) return;
    let failures = 0;
    let events: readonly SandboxEvent[] = [];
    let observedStatus: string | undefined;
    const observer = observeRead(async (signal) => {
      try {
        const next = await readPreparation(client, agentId, signal, stateRef, setState, wake);
        events = await readSandboxHistory(client, next.detail.sandbox.id, signal, events);
        if (signal.aborted) return;
        setState((current) => {
          if (current.status !== 'ready' || current.detail.id !== agentId) return current;
          const conversation = refreshConversation(current.conversation, next);
          liveCursor.current = conversation.cursor;
          return { status: 'ready', ...next, conversation, sandboxEvents: events };
        });
        setActionError((current) =>
          current === 'Preparation progress could not be refreshed. Reconnecting.'
            ? undefined
            : current,
        );
        if (next.detail.status !== 'starting' && observedStatus !== next.detail.status)
          hierarchyChanged.current?.();
        observedStatus = next.detail.status;
        failures = 0;
        return { next: 1500, cooldown: 1500 };
      } catch (error) {
        if (!signal.aborted && !onIdentityChange(error)) {
          setActionError('Preparation progress could not be refreshed. Reconnecting.');
          return retrySchedule(error, ++failures, 5000);
        }
      }
    });
    return () => observer.dispose();
  }, [
    agentId,
    client,
    pending,
    provisioning,
    setState,
    stateRef,
    setActionError,
    liveCursor,
    hierarchyChanged,
    onIdentityChange,
    wake,
  ]);
}
