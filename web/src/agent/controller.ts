import { useCallback, useEffect, useRef, useState } from 'react';
import { ApiError } from '@chronoai/cma-sdk';
import { loadAgent, transientReadFailure, type ReadyAgent } from './loadAgent';
export { loadAgent, type ReadyAgent } from './loadAgent';
import type { CmaWebClient } from '../api/client';
import { CommandKeys } from '../api/client';
import type { AgentLiveEvent, AgentSummary, SandboxEvent } from '../api/types';
import { readSandboxHistory } from '../sandbox/eventHistory';
import { currentAgentStatus, sameAgent } from './status';
import { mergeTranscriptSnapshot } from './chronology';
import {
  useSandboxInput,
  type SandboxInputAvailability,
  type SandboxResourceAvailability,
} from './sandboxInput';
import { foldAgentEvent, foldLiveEvent, type ConversationState } from './conversation';

export type AgentResourceState =
  | { readonly status: 'idle' }
  | { readonly status: 'loading' }
  | { readonly status: 'error'; readonly message: string }
  | ({ readonly status: 'ready' } & ReadyAgent);

export type AgentCommand = 'send' | 'interrupt' | 'approval' | 'stop' | 'delete';
export type ApprovalDecision = 'allow' | 'deny' | 'abort';

export interface AgentController {
  readonly state: AgentResourceState;
  readonly inputAvailability: SandboxInputAvailability;
  readonly resourceAvailability: SandboxResourceAvailability;
  readonly progressObservation?: ReturnType<typeof useSandboxInput>['progressObservation'];
  readonly pending?: AgentCommand;
  readonly actionError?: string;
  readonly reload: () => void;
  readonly clearActionError: () => void;
  readonly send: (content: string, onAccepted?: () => void) => Promise<void>;
  readonly interrupt: () => Promise<void>;
  readonly approve: (approvalId: string, decision: ApprovalDecision) => Promise<void>;
  readonly stop: () => Promise<void>;
  readonly delete: () => Promise<void>;
}

interface AgentControllerOptions {
  readonly summary?: AgentSummary;
  readonly onHierarchyChange?: () => void;
  readonly onDeleted?: () => void;
}

function errorMessage(error: unknown): string {
  if (error instanceof ApiError && error.code === 'sandbox_checkpoint_in_progress')
    return 'Progress is being saved. Your input was not accepted. Try again after saving finishes.';
  return error instanceof Error ? error.message : 'The Agent operation did not complete.';
}

function refreshConversation(
  current: ConversationState,
  next: Awaited<ReturnType<typeof loadAgent>>,
): ConversationState {
  if (current.cursor > next.conversation.cursor) return current;
  const folded = next.durableEvents.reduce(foldAgentEvent, current);
  return {
    ...mergeTranscriptSnapshot(folded, next.conversation, current.cursor),
    usage:
      (next.conversation.usage?.throughSeq ?? 0) >= (folded.usage?.throughSeq ?? 0)
        ? next.conversation.usage
        : folded.usage,
  };
}

function runId(): string {
  return typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `run-${String(Date.now())}-${String(Math.random()).slice(2)}`;
}

export function useAgentController(
  client: CmaWebClient,
  agentId: string | undefined,
  options: AgentControllerOptions = {},
): AgentController {
  const [revision, setRevision] = useState(0);
  const [replayRevision, setReplayRevision] = useState(0);
  const [state, setState] = useState<AgentResourceState>(
    agentId === undefined ? { status: 'idle' } : { status: 'loading' },
  );
  const { inputAvailability, resourceAvailability, progressObservation } = useSandboxInput(
    client,
    agentId,
    state.status === 'ready' && state.detail.id === agentId ? state.detail.sandbox.id : undefined,
  );
  const activeTurn =
    state.status === 'ready' &&
    state.detail.id === agentId &&
    state.conversation.activeRunId !== undefined;
  const [pending, setPending] = useState<AgentCommand>();
  const [actionError, setActionError] = useState<string>();
  const [replayError, setReplayError] = useState<string>();
  const [keys] = useState(() => new CommandKeys());
  const actionAbort = useRef<AbortController | undefined>(undefined);
  const actionKind = useRef<AgentCommand | undefined>(undefined);
  const replayAbort = useRef<AbortController | undefined>(undefined);
  const liveCursor = useRef(0);
  const hierarchyChanged = useRef(options.onHierarchyChange);
  useEffect(() => {
    hierarchyChanged.current = options.onHierarchyChange;
  }, [options.onHierarchyChange]);

  const summary = options.summary;
  const detail = state.status === 'ready' ? state.detail : undefined;
  useEffect(() => {
    if (summary === undefined || detail === undefined || summary.id !== agentId) return;
    let cancelled = false;
    // Reuse fresh tree metadata without reloading or replacing conversation history.
    queueMicrotask(() => {
      if (cancelled) return;
      setState((current) => {
        if (
          current.status !== 'ready' ||
          !sameAgent(current.detail, summary) ||
          !(Date.parse(summary.updatedAt) >= Date.parse(current.detail.updatedAt))
        )
          return current;
        const status =
          Date.parse(summary.updatedAt) > Date.parse(current.detail.updatedAt)
            ? summary.status
            : current.detail.status;
        const title = summary.title?.trim() ? summary.title : current.detail.title;
        if (current.detail.status === status && current.detail.title === title) return current;
        return {
          ...current,
          detail: {
            ...current.detail,
            status,
            title,
            updatedAt: summary.updatedAt,
          },
        };
      });
    });
    return () => {
      cancelled = true;
    };
  }, [agentId, detail, summary]);

  const applyLive = useCallback(
    (event: AgentLiveEvent) => {
      if (agentId === undefined) return;
      setState((current) => {
        if (current.status !== 'ready' || current.detail.id !== agentId) return current;
        const conversation = foldLiveEvent(current.conversation, event, agentId);
        liveCursor.current = conversation.cursor;
        return { ...current, conversation };
      });
    },
    [agentId],
  );

  useEffect(() => {
    const controller = new AbortController();
    queueMicrotask(() => {
      if (controller.signal.aborted) return;
      setActionError(undefined);
      setReplayError(undefined);
      setPending(undefined);
      if (agentId === undefined) {
        setState({ status: 'idle' });
        return;
      }
      setState({ status: 'loading' });
      void loadAgent(client, agentId, controller.signal, true, (ready) => {
        if (!controller.signal.aborted) setState({ status: 'ready', ...ready });
      })
        .then((ready) => {
          if (controller.signal.aborted) return;
          liveCursor.current = ready.conversation.cursor;
          setState({ status: 'ready', ...ready });
        })
        .catch((error: unknown) => {
          if (!controller.signal.aborted)
            setState({ status: 'error', message: errorMessage(error) });
        });
    });
    return () => {
      controller.abort();
    };
  }, [agentId, applyLive, client, revision]);

  const historyUnavailable =
    state.status === 'ready' && state.detail.id === agentId && state.historyUnavailable === true;
  const historyLoading =
    state.status === 'ready' && state.detail.id === agentId && state.historyLoading === true;
  const historyIncomplete = historyLoading || historyUnavailable;
  useEffect(() => {
    if (!historyUnavailable || agentId === undefined) return;
    const request = new AbortController();
    let timer: ReturnType<typeof setTimeout>;
    let retryDelay = 5000;
    const recover = async () => {
      try {
        const ready = await loadAgent(client, agentId, request.signal);
        if (request.signal.aborted) return;
        liveCursor.current = ready.conversation.cursor;
        setState({ status: 'ready', ...ready });
      } catch (error) {
        if (request.signal.aborted) return;
        if (transientReadFailure(error)) {
          retryDelay = Math.min(retryDelay * 2, 30000);
          timer = setTimeout(() => void recover(), retryDelay);
        } else setState({ status: 'error', message: errorMessage(error) });
      }
    };
    timer = setTimeout(() => void recover(), 5000);
    return () => {
      request.abort();
      clearTimeout(timer);
    };
  }, [agentId, client, historyUnavailable]);

  const replayEnabled =
    agentId !== undefined &&
    state.status === 'ready' &&
    state.detail.id === agentId &&
    !historyIncomplete &&
    pending !== 'send' &&
    pending !== 'approval';

  const provisioning =
    state.status === 'ready' &&
    state.detail.id === agentId &&
    !historyIncomplete &&
    (state.detail.status === 'starting' ||
      (state.detail.firstMessage !== undefined &&
        !['completed', 'failed', 'recovery_required'].includes(state.detail.firstMessage.state)));
  useEffect(() => {
    if (!provisioning || agentId === undefined || pending !== undefined) return;
    const request = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    let events: readonly SandboxEvent[] = [];
    let observedStatus: string | undefined;
    const poll = async () => {
      try {
        const next = await loadAgent(client, agentId, request.signal);
        events = await readSandboxHistory(client, next.detail.sandbox.id, request.signal, events);
        if (request.signal.aborted) return;
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
        timer = setTimeout(() => void poll(), 1500);
      } catch {
        if (!request.signal.aborted) {
          setActionError('Preparation progress could not be refreshed. Reconnecting.');
          timer = setTimeout(() => void poll(), 5000);
        }
      }
    };
    timer = setTimeout(() => void poll(), 1500);
    return () => {
      request.abort();
      clearTimeout(timer);
    };
  }, [agentId, client, pending, provisioning]);

  useEffect(() => {
    if (!replayEnabled || agentId === undefined) return;
    const controller = new AbortController();
    replayAbort.current = controller;
    queueMicrotask(() => {
      if (!controller.signal.aborted) setReplayError(undefined);
    });
    void client
      .replayAgent(agentId, {
        after: liveCursor.current,
        signal: controller.signal,
        onEvent: (event) => {
          if (!controller.signal.aborted) applyLive(event);
        },
      })
      .catch((error: unknown) => {
        if (controller.signal.aborted) return;
        setReplayError(`Live Agent updates are unavailable. ${errorMessage(error)}`);
      });
    return () => {
      controller.abort();
      if (replayAbort.current === controller) replayAbort.current = undefined;
    };
  }, [agentId, applyLive, client, replayEnabled, replayRevision]);

  useEffect(() => {
    return () => {
      const controller = actionAbort.current;
      controller?.abort();
      if (actionAbort.current === controller) {
        actionAbort.current = undefined;
        actionKind.current = undefined;
      }
    };
  }, [agentId, client]);

  const reload = useCallback(() => {
    setActionError(undefined);
    setReplayError(undefined);
    setState(agentId === undefined ? { status: 'idle' } : { status: 'loading' });
    setRevision((value) => value + 1);
  }, [agentId]);

  const run = useCallback(
    async (
      approval: { readonly id: string; readonly decision: ApprovalDecision } | string,
      onAccepted?: () => void,
    ) => {
      if (
        agentId === undefined ||
        actionAbort.current !== undefined ||
        historyIncomplete ||
        !inputAvailability.allowed
      )
        return;
      replayAbort.current?.abort();
      const controller = new AbortController();
      actionAbort.current = controller;
      const approving = typeof approval !== 'string';
      actionKind.current = approving ? 'approval' : 'send';
      setPending(approving ? 'approval' : 'send');
      setActionError(undefined);
      let accepted = false;
      try {
        await client.runAgent(
          {
            threadId: agentId,
            runId: runId(),
            ...(approving
              ? {
                  resume: [
                    {
                      interruptId: approval.id,
                      status: 'resolved' as const,
                      payload: { decision: approval.decision },
                    },
                  ],
                }
              : { messages: [{ role: 'user' as const, content: approval }] }),
          },
          {
            signal: controller.signal,
            onEvent: (event) => {
              if (controller.signal.aborted || event.threadId !== agentId) return;
              if (!accepted && event.type !== 'RUN_ERROR') {
                accepted = true;
                onAccepted?.();
              }
              applyLive(event);
            },
          },
        );
      } catch (error: unknown) {
        if (!controller.signal.aborted) setActionError(errorMessage(error));
      } finally {
        if (actionAbort.current === controller) {
          actionAbort.current = undefined;
          actionKind.current = undefined;
          setPending(undefined);
          setReplayRevision((value) => value + 1);
        }
      }
    },
    [agentId, applyLive, client, historyIncomplete, inputAvailability.allowed],
  );

  const command = useCallback(
    async (
      name: Exclude<AgentCommand, 'send' | 'approval'>,
      operation: (id: string, key: string, signal: AbortSignal) => Promise<void>,
    ) => {
      const mayPreemptSend = name === 'interrupt' && actionKind.current === 'send';
      if (historyLoading) return;
      if (name === 'interrupt' && !activeTurn) return;
      if (agentId === undefined || (actionAbort.current !== undefined && !mayPreemptSend)) {
        return;
      }
      if (name === 'interrupt') actionAbort.current?.abort();
      const scope = `agent:${agentId}:${name}`;
      const controller = new AbortController();
      actionAbort.current = controller;
      actionKind.current = name;
      setPending(name);
      setActionError(undefined);
      try {
        await operation(agentId, keys.for(scope), controller.signal);
        keys.settle(scope);
        options.onHierarchyChange?.();
        if (name === 'delete') options.onDeleted?.();
        else reload();
      } catch (error: unknown) {
        if (!controller.signal.aborted) setActionError(errorMessage(error));
      } finally {
        if (actionAbort.current === controller) {
          actionAbort.current = undefined;
          actionKind.current = undefined;
          setPending(undefined);
          if (name === 'interrupt') setReplayRevision((value) => value + 1);
        }
      }
    },
    [activeTurn, agentId, historyLoading, keys, options, reload],
  );

  const displayedStatus =
    state.status === 'ready'
      ? currentAgentStatus(state.detail, state.conversation, summary)
      : undefined;
  return {
    state:
      state.status === 'ready' &&
      displayedStatus !== undefined &&
      displayedStatus !== state.detail.status
        ? {
            ...state,
            detail: { ...state.detail, status: displayedStatus },
          }
        : state,
    inputAvailability,
    resourceAvailability,
    progressObservation,
    pending,
    actionError: actionError ?? replayError,
    reload,
    clearActionError: () => {
      setActionError(undefined);
      setReplayError(undefined);
    },
    send: (content, onAccepted) => run(content, onAccepted),
    approve: (approvalId, decision) => run({ id: approvalId, decision }),
    interrupt: () =>
      command('interrupt', (id, key, signal) =>
        client.interruptAgent(id, { idempotencyKey: key, signal }),
      ),
    stop: () =>
      command('stop', (id, key, signal) => client.stopAgent(id, { idempotencyKey: key, signal })),
    delete: () =>
      command('delete', (id, key, signal) =>
        client.deleteAgent(id, { idempotencyKey: key, signal }),
      ),
  };
}
