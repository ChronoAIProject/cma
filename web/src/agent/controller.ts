import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { ApiError } from '@chronoai/cma-sdk';
import { loadAgent, transientReadFailure, type AgentLoadLineage } from './loadAgent';
export { loadAgent, type ReadyAgent } from './loadAgent';
import type { CmaWebClient } from '../api/client';
import { CommandKeys } from '../api/client';
import type { AgentLiveEvent, SandboxEvent } from '../api/types';
import { readSandboxHistory } from '../sandbox/eventHistory';
import { currentAgentStatus, sameAgent } from './status';
import { useSandboxInput } from './sandboxInput';
import { foldLiveEvent } from './conversation';
import { useRetainedAgentState } from './retainedState';
import { reopenAgent, refreshConversation } from './reopenAgent';
import { foldV2Frame } from './transport/projection';
import { followV2Agent } from './transport/replay';
import { useAgentActions } from './actions';
import { IdentityChangedError, isIdentityChanged } from './transport/identity';
import { useControlStatus } from './transport/controlStatus';
import { useQueuedInputs } from './transport/queuedInputsStatus';
import { presentAgent } from './presentation';
import { mergeRuntimeMetadata } from './runtimeMetadata';
import type { AgentCommand, AgentController, AgentControllerOptions } from './controllerTypes';
export type {
  AgentCommand,
  AgentController,
  AgentResourceState,
  ApprovalDecision,
} from './controllerTypes';

function errorMessage(error: unknown): string {
  if (isIdentityChanged(error)) return new IdentityChangedError().message;
  if (error instanceof ApiError && error.code === 'sandbox_checkpoint_in_progress')
    return 'Progress is being saved. Your input was not accepted. Try again after saving finishes.';
  return error instanceof Error ? error.message : 'The Agent operation did not complete.';
}

export function useAgentController(
  client: CmaWebClient,
  agentId: string | undefined,
  options: AgentControllerOptions = {},
): AgentController {
  const suspended = options.suspended === true;
  const {
    agentId: lineageAgentId,
    workspaceId: lineageWorkspaceId,
    sandboxId: lineageSandboxId,
  } = options.authorizedLineage ?? {};
  const lineage = useMemo(
    () =>
      !suspended &&
      options.viewerId !== undefined &&
      lineageAgentId === agentId &&
      lineageAgentId !== undefined &&
      lineageWorkspaceId !== undefined &&
      lineageSandboxId !== undefined
        ? { agentId: lineageAgentId, workspaceId: lineageWorkspaceId, sandboxId: lineageSandboxId }
        : undefined,
    [agentId, suspended, options.viewerId, lineageAgentId, lineageWorkspaceId, lineageSandboxId],
  );
  const [revision, setRevision] = useState(0);
  const [replayRevision, setReplayRevision] = useState(0);
  const [requestRevision, setRequestRevision] = useState(0);
  const [state, setState, stateRef] = useRetainedAgentState(client, options.viewerId, agentId);
  const loaded = useRef<
    | { client: CmaWebClient; agentId?: string; revision: number; lineage?: AgentLoadLineage }
    | undefined
  >(undefined);
  const { inputAvailability, resourceAvailability, progressObservation } = useSandboxInput(
    client,
    suspended ? undefined : agentId,
    state.status === 'ready' && state.detail.id === agentId
      ? state.detail.sandbox.id
      : lineage?.sandboxId,
    state.status === 'ready' &&
      state.detail.id === agentId &&
      state.detail.status === 'starting' &&
      state.v2 !== undefined,
    state.status === 'ready' && state.detail.id === agentId && state.v2 !== undefined
      ? state.detail.bootstrap
      : undefined,
    options.viewerId,
    options.viewerId !== undefined &&
      (state.status === 'ready' && state.detail.id === agentId
        ? state.v2 !== undefined
        : lineage !== undefined),
  );
  const activeTurn =
    state.status === 'ready' &&
    state.detail.id === agentId &&
    state.conversation.activeRunId !== undefined;
  const [pending, setPending] = useState<AgentCommand>();
  const [actionError, setActionError] = useState<string>();
  const [replayError, setReplayError] = useState<string>();
  const [keys] = useState(() => new CommandKeys());
  const actionAbortRef = useRef<AbortController | undefined>(undefined);
  const actionKindRef = useRef<AgentCommand | undefined>(undefined);
  const replayAbortRef = useRef<AbortController | undefined>(undefined);
  const liveCursor = useRef(0);
  const onIdentityChange = useCallback(
    (error: unknown) => {
      if (!isIdentityChanged(error)) return false;
      actionAbortRef.current?.abort();
      replayAbortRef.current?.abort();
      setPending(undefined);
      setActionError(undefined);
      setReplayError(undefined);
      setState({ status: 'error', message: new IdentityChangedError().message });
      return true;
    },
    [setState],
  );
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
        const runtimeMetadata = mergeRuntimeMetadata(
          current.detail.runtimeMetadata,
          summary.runtimeMetadata,
        );
        if (
          current.detail.status === status &&
          current.detail.title === title &&
          current.detail.runtimeMetadata === runtimeMetadata
        )
          return current;
        return {
          ...current,
          detail: {
            ...current.detail,
            status,
            title,
            runtimeMetadata,
            updatedAt: summary.updatedAt,
          },
        };
      });
    });
    return () => {
      cancelled = true;
    };
  }, [agentId, detail, summary, setState]);

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
    [agentId, setState],
  );

  useEffect(() => {
    if (suspended) return;
    const controller = new AbortController();
    queueMicrotask(() => {
      if (controller.signal.aborted) return;
      setActionError(undefined);
      setReplayError(undefined);
      setPending(undefined);
      if (agentId === undefined) {
        loaded.current = undefined;
        setState({ status: 'idle' });
        return;
      }
      if (
        loaded.current?.client === client &&
        loaded.current.agentId === agentId &&
        loaded.current.revision === revision &&
        loaded.current.lineage === lineage &&
        stateRef.current.status === 'ready'
      )
        return;
      const retained = stateRef.current;
      const reopen =
        retained.status === 'ready' && !retained.historyLoading && !retained.historyUnavailable;
      loaded.current = { client, agentId, revision, lineage };
      if (!reopen) setState({ status: 'loading' });
      else liveCursor.current = retained.conversation.cursor;
      void (
        reopen
          ? reopenAgent(client, retained, controller.signal, lineage)
          : loadAgent(
              client,
              agentId,
              controller.signal,
              true,
              (ready) => {
                if (!controller.signal.aborted) setState({ status: 'ready', ...ready });
              },
              lineage,
            )
      )
        .then((ready) => {
          if (controller.signal.aborted) return;
          setState((current) => {
            const conversation =
              reopen && current.status === 'ready' ? current.conversation : ready.conversation;
            liveCursor.current = conversation.cursor;
            return { status: 'ready', ...ready, conversation };
          });
        })
        .catch((error: unknown) => {
          if (!controller.signal.aborted)
            setState({ status: 'error', message: errorMessage(error) });
        });
    });
    return () => {
      controller.abort();
    };
  }, [agentId, applyLive, client, lineage, revision, setState, stateRef, suspended]);

  const historyUnavailable =
    state.status === 'ready' && state.detail.id === agentId && state.historyUnavailable === true;
  const historyLoading =
    state.status === 'ready' && state.detail.id === agentId && state.historyLoading === true;
  const historyIncomplete = historyLoading || historyUnavailable;
  const v2 =
    !suspended && state.status === 'ready' && state.detail.id === agentId ? state.v2 : undefined;
  useEffect(() => {
    if (suspended || !historyUnavailable || agentId === undefined) return;
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
  }, [agentId, client, historyUnavailable, setState, suspended]);

  const replayEnabled =
    !suspended &&
    agentId !== undefined &&
    state.status === 'ready' &&
    state.detail.id === agentId &&
    !historyIncomplete &&
    (v2 !== undefined || (pending !== 'send' && pending !== 'approval'));

  const provisioning =
    !suspended &&
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
      } catch (error) {
        if (!request.signal.aborted && !onIdentityChange(error)) {
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
  }, [agentId, client, pending, provisioning, setState, onIdentityChange]);

  useEffect(() => {
    if (!replayEnabled || agentId === undefined) return;
    const controller = new AbortController();
    replayAbortRef.current = controller;
    queueMicrotask(() => {
      if (!controller.signal.aborted) setReplayError(undefined);
    });
    const replay =
      v2 === undefined
        ? client.replayAgent(agentId, {
            after:
              stateRef.current.status === 'ready'
                ? stateRef.current.conversation.cursor
                : liveCursor.current,
            signal: controller.signal,
            onEvent: (event) => {
              if (!controller.signal.aborted) applyLive(event);
            },
          })
        : followV2Agent(
            v2,
            stateRef.current.status === 'ready' ? stateRef.current.conversation.agentCursor! : '',
            controller.signal,
            (frame) => {
              const current = stateRef.current;
              if (current.status !== 'ready' || current.detail.id !== agentId)
                throw new Error('The Agent view no longer accepts live updates.');
              const conversation = foldV2Frame(current.conversation, frame, agentId);
              liveCursor.current = conversation.cursor;
              setState({ ...current, conversation });
            },
            setReplayError,
          );
    void replay.catch((error: unknown) => {
      if (controller.signal.aborted || onIdentityChange(error)) return;
      setReplayError(`Live Agent updates are unavailable. ${errorMessage(error)}`);
    });
    return () => {
      controller.abort();
      if (replayAbortRef.current === controller) replayAbortRef.current = undefined;
    };
  }, [
    agentId,
    applyLive,
    client,
    replayEnabled,
    replayRevision,
    v2,
    setState,
    stateRef,
    onIdentityChange,
  ]);

  useLayoutEffect(() => {
    return () => {
      const controller = actionAbortRef.current;
      controller?.abort();
      if (actionAbortRef.current === controller) {
        actionAbortRef.current = undefined;
        actionKindRef.current = undefined;
      }
    };
  }, [agentId, client, options.viewerId, suspended]);

  const reload = useCallback(() => {
    setActionError(undefined);
    setReplayError(undefined);
    setState(agentId === undefined ? { status: 'idle' } : { status: 'loading' });
    setRevision((value) => value + 1);
  }, [agentId, setState]);

  const { run, command, recoverPending } = useAgentActions({
    agentId: suspended ? undefined : agentId,
    client,
    historyIncomplete: historyIncomplete || suspended,
    activeTurn,
    inputAvailability,
    actionAbortRef,
    actionKindRef,
    replayAbortRef,
    stateRef,
    keys,
    v2,
    setPending,
    setActionError,
    setReplayRevision,
    setRequestRevision,
    applyLive,
    reload,
    errorMessage,
    onIdentityChange,
    options,
  });

  const displayedStatus =
    state.status === 'ready'
      ? currentAgentStatus(state.detail, state.conversation, summary)
      : undefined;
  const retained = v2?.pending();
  const controlNotice = useControlStatus(v2, onIdentityChange);
  const queuedInputs = useQueuedInputs(
    v2,
    !historyIncomplete && !suspended,
    activeTurn,
    `${state.status === 'ready' ? state.conversation.cursor : 0}:${requestRevision}`,
    onIdentityChange,
  );
  return {
    queuedInputs: v2 === undefined ? undefined : queuedInputs,
    pendingMessage: retained?.input,
    acceptedMessage: retained?.accepted,
    acceptedSteeringMessage: retained?.acceptedSteering,
    pendingControl: retained?.control,
    controlNotice,
    controlsUnavailable:
      suspended ||
      (v2 !== undefined &&
        state.status === 'ready' &&
        state.conversation.responseBinding === undefined),
    recoverPending: v2 === undefined ? undefined : recoverPending,
    state: presentAgent(state, displayedStatus),
    inputAvailability: suspended
      ? {
          allowed: false,
          notice:
            'Checking Agent access. Your conversation and draft are retained; actions are unavailable until access is verified.',
        }
      : inputAvailability,
    resourceAvailability: suspended
      ? { state: 'unavailable', notice: 'Agent access is being checked. Live resources are held.' }
      : resourceAvailability,
    progressObservation,
    pending,
    actionError: actionError ?? replayError,
    reload,
    clearActionError: () => {
      setActionError(undefined);
      setReplayError(undefined);
    },
    send: (content, onAccepted, onReceiptObserved, onRefused) =>
      run(content, onAccepted, onReceiptObserved, onRefused),
    steer:
      v2 === undefined
        ? undefined
        : (content, onAccepted, onReceiptObserved, onRefused) =>
            run(content, onAccepted, onReceiptObserved, onRefused, true),
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
