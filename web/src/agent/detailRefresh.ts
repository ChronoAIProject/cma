import {
  useCallback,
  useLayoutEffect,
  useRef,
  type Dispatch,
  type RefObject,
  type SetStateAction,
} from 'react';
import type { CmaWebClient } from '../api/client';
import type { AgentResourceState } from './controllerTypes';
import { transientReadFailure, type AgentLoadLineage } from './loadAgent';
import { reopenAgent } from './reopenAgent';
import { agentReadRetryAt } from './controllerError';
import { WorkspaceReadUnavailable } from '../api/rest/workspaceRead';
import { rateLimitDelay } from './transport/readRetry';

interface Options {
  readonly client: CmaWebClient;
  readonly agentId?: string;
  readonly viewerId?: string;
  readonly suspended: boolean;
  readonly ready: boolean;
  readonly revision: number;
  readonly lineage?: AgentLoadLineage;
  readonly stateRef: RefObject<AgentResourceState>;
  readonly setState: Dispatch<SetStateAction<AgentResourceState>>;
  readonly reload: () => void;
  readonly onIdentityChange: (error: unknown) => boolean;
}

/** Event-driven metadata reads never restart history hydration or the live subscription. */
export function useAgentDetailRefresh({
  client,
  agentId,
  viewerId,
  suspended,
  ready,
  revision,
  lineage,
  stateRef,
  setState,
  reload,
  onIdentityChange,
}: Options) {
  const metadataVersion = useRef(0);
  const trigger = useRef<(() => void) | undefined>(undefined);
  useLayoutEffect(() => {
    if (suspended || !ready || agentId === undefined) return;
    let active = true;
    let inFlight: AbortController | undefined;
    let queued = false;
    let retryTimer: ReturnType<typeof setTimeout> | undefined;
    let retryAt = 0;
    const read = async (retry = false) => {
      const retained = stateRef.current;
      if (!active || retained.status !== 'ready' || retained.detail.id !== agentId) return;
      const controller = new AbortController();
      inFlight = controller;
      queued = false;
      try {
        const next = await reopenAgent(client, retained, controller.signal, lineage);
        if (!active || controller.signal.aborted) return;
        setState((current) => {
          if (
            current.status !== 'ready' ||
            current.detail.id !== next.detail.id ||
            current.workspace.id !== next.workspace.id ||
            current.detail.workspace.id !== next.detail.workspace.id ||
            current.detail.sandbox.id !== next.detail.sandbox.id ||
            current.v2 !== retained.v2
          )
            return current;
          metadataVersion.current += 1;
          return {
            ...current,
            detail: next.detail,
            workspace: next.workspace,
            ...(current.workspaceUnavailable ? { workspaceUnavailable: false } : {}),
          };
        });
      } catch (error: unknown) {
        if (!active || controller.signal.aborted) return;
        // Cancel any sibling projection read when Promise.all rejects.
        controller.abort();
        queued = false;
        if (onIdentityChange(error)) return;
        if (transientReadFailure(error)) {
          if (error instanceof WorkspaceReadUnavailable) {
            setState((current) => {
              if (
                current.status !== 'ready' ||
                current.detail.id !== agentId ||
                current.workspace.id !== retained.workspace.id ||
                current.v2 !== retained.v2 ||
                current.workspaceUnavailable
              )
                return current;
              metadataVersion.current += 1;
              return { ...current, workspaceUnavailable: true };
            });
          }
          const delay = rateLimitDelay(error, 5000);
          // Keep the server floor even after the single automatic retry is exhausted.
          retryAt = Date.now() + delay;
          console.warn('[agent.detail] Metadata refresh unavailable.', {
            retryScheduled: !retry,
            cooldownMs: delay,
          });
          if (!retry)
            retryTimer = setTimeout(() => {
              retryTimer = undefined;
              void read(true);
            }, delay);
        } else {
          console.warn('[agent.detail] Metadata access changed; revalidating the Agent view.');
          reload();
        }
      } finally {
        inFlight = undefined;
        if (active && queued && retryTimer === undefined) void read();
      }
    };
    trigger.current = () => {
      // Triggers during a backoff share its single retry rather than bypassing it.
      if (retryTimer !== undefined || Date.now() < retryAt) return;
      if (inFlight) queued = true;
      else void read();
    };
    return () => {
      active = false;
      trigger.current = undefined;
      inFlight?.abort();
      clearTimeout(retryTimer);
    };
  }, [
    client,
    agentId,
    viewerId,
    suspended,
    ready,
    revision,
    lineage,
    stateRef,
    setState,
    reload,
    onIdentityChange,
  ]);
  const refreshDetail = useCallback(() => trigger.current?.(), []);
  return { refreshDetail, metadataVersion };
}

/** Explicit Retry keeps the established full-load and cooldown behavior. */
export function useAgentReload(
  agentId: string | undefined,
  stateRef: RefObject<AgentResourceState>,
  setState: Dispatch<SetStateAction<AgentResourceState>>,
  setRevision: Dispatch<SetStateAction<number>>,
  setActionError: Dispatch<SetStateAction<string | undefined>>,
  setStreamError: Dispatch<SetStateAction<string | undefined>>,
) {
  return useCallback(() => {
    if (agentReadRetryAt(stateRef.current) > Date.now()) return;
    setActionError(undefined);
    setStreamError(undefined);
    setState(agentId === undefined ? { status: 'idle' } : { status: 'loading' });
    setRevision((value) => value + 1);
  }, [agentId, setState, stateRef, setRevision, setActionError, setStreamError]);
}
