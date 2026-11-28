import { useAgentDetailRefresh, useAgentReload } from './detailRefresh';
import {
  useHistoryPublication,
  useHistoryRetry,
  retainCurrentMetadata,
} from './historyPublication';
import * as controlReadiness from './controlReadiness';
import { observeRead } from './transport/readObserver';
import { retrySchedule } from './transport/readRetry';
import { useAgentTitle } from './useAgentTitle';
import { usePreparationProgress } from './preparationProgress';
import { queueObservationRevision } from './queueObservationRevision';
import { ReadinessWake } from './readinessWait';
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react';
import { useControllerMode } from './controllerMode';
import { agentReadRetryAt, errorMessage, retainHistoryOnFailure } from './controllerError';
import { loadAgent, transientReadFailure, type AgentLoadLineage } from './loadAgent';
export { loadAgent, type ReadyAgent } from './loadAgent';
import { CommandKeys, type CmaWebClient } from '../api/client';
import { WorkspaceReadUnavailable } from '../api/rest/workspaceRead';
import type { AgentLiveEvent } from '../api/types';
import { accessCheckedInputAvailability, useSandboxInput } from './sandboxInput';
import { ConversationHydration } from './transport/hydration';
import { useRetainedAgentState } from './retainedState';
import { reopenAgent } from './reopenAgent';
import {
  observeApprovalProjection,
  approvalProjectionView,
} from './transport/approvalProjectionObserver';
import { followV2Agent } from './transport/replay';
import { useAgentActions } from './actions';
import { lifecycleCommands } from './lifecycleCommands';
import { useAgentActionError, useAgentIdentityChange } from './actionError';
import { useControlStatus } from './transport/controlStatus';
import { useFirstReplyStop } from './transport/firstReplyStop';
import { useQueuedInputs } from './transport/queuedInputsStatus';
import { usePresentedAgent, usePresentedRetainedInputs } from './presentation';
import { useAgentSummary } from './useAgentSummary';
import type { AgentCommand, AgentController, AgentControllerOptions } from './controllerTypes';
export type { AgentCommand, AgentController } from './controllerTypes';
export type { AgentResourceState, ApprovalDecision } from './controllerTypes';

export function useAgentController(
  client: CmaWebClient,
  agentId: string | undefined,
  options: AgentControllerOptions = {},
): AgentController {
  const { suspended, lineage, retained } = useControllerMode(client, agentId, options);
  const [revision, setRevision] = useState(0);
  const [replayRevision, setReplayRevision] = useState(0);
  const [requestRevision, setRequestRevision] = useState(0);
  const [readinessWake] = useState(() => new ReadinessWake());
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
  const responseTarget = activeTurn ? state.conversation.responseBinding : undefined;
  const [pending, setPending] = useState<AgentCommand>();
  const [actionError, setActionError, recoverInputError] = useAgentActionError();
  const [streamError, setStreamError] = useState<string>();
  const [keys] = useState(() => new CommandKeys());
  const actionAbortRef = useRef<AbortController | undefined>(undefined);
  const actionKindRef = useRef<AgentCommand | undefined>(undefined);
  const replayAbortRef = useRef<AbortController | undefined>(undefined);
  const liveCursor = useRef(0);
  const hydration = useRef(new ConversationHydration());
  const onIdentityChange = useAgentIdentityChange(
    actionAbortRef,
    replayAbortRef,
    setPending,
    setActionError,
    setStreamError,
    setState,
  );
  const hierarchyChanged = useRef(options.onHierarchyChange);
  useEffect(() => {
    hierarchyChanged.current = options.onHierarchyChange;
  }, [options.onHierarchyChange]);
  const summary = options.summary;
  const detail = state.status === 'ready' ? state.detail : undefined;
  useAgentSummary(agentId, detail, summary, setState);
  const applyLive = useCallback(
    (event: AgentLiveEvent) => {
      if (agentId === undefined) return;
      setState((current) => {
        if (current.status !== 'ready' || current.detail.id !== agentId) return current;
        const conversation = hydration.current.apply(
          current.conversation,
          event,
          agentId,
          current.historyLoading === true || current.historyUnavailable === true,
        );
        liveCursor.current = conversation.cursor;
        return { ...current, conversation };
      });
    },
    [agentId, setState],
  );
  const reload = useAgentReload(
    agentId,
    stateRef,
    setState,
    setRevision,
    setActionError,
    setStreamError,
  );
  const { refreshDetail, metadataVersion } = useAgentDetailRefresh({
    client,
    agentId,
    viewerId: options.viewerId,
    suspended,
    revision,
    lineage,
    ready: state.status === 'ready',
    stateRef,
    setState,
    reload,
    onIdentityChange,
  });
  const publishHistory = useHistoryPublication(agentId, setState, hydration, liveCursor);
  useEffect(() => {
    if (suspended) return;
    let failures = 0;
    const observer = observeRead(
      async (lifetimeSignal) => {
        const metadataAtStart = metadataVersion.current;
        const operation = new AbortController();
        const signal = AbortSignal.any([lifetimeSignal, operation.signal]);
        setActionError(undefined);
        setStreamError(undefined);
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
        hydration.current.reset(reopen ? retained.conversation : undefined);
        if (!reopen && retained.status !== 'ready') setState({ status: 'loading' });
        else if (reopen) liveCursor.current = retained.conversation.cursor;
        return (
          reopen
            ? reopenAgent(client, retained, signal, lineage)
            : loadAgent(
                client,
                agentId,
                signal,
                true,
                (ready) => {
                  if (!signal.aborted)
                    publishHistory(ready, false, metadataVersion.current !== metadataAtStart);
                },
                lineage,
                (ready) => {
                  if (!signal.aborted)
                    publishHistory(ready, true, metadataVersion.current !== metadataAtStart);
                },
              )
        )
          .then((ready) => {
            if (signal.aborted) return;
            setState((current) => {
              const conversation =
                reopen && current.status === 'ready'
                  ? current.conversation
                  : hydration.current.merge(ready.conversation, agentId, !ready.historyUnavailable);
              liveCursor.current = conversation.cursor;
              return {
                status: 'ready',
                ...(metadataVersion.current === metadataAtStart
                  ? ready
                  : retainCurrentMetadata(ready, current)),
                conversation,
              };
            });
            return undefined;
          })
          .catch((error: unknown) => {
            if (!lifetimeSignal.aborted) {
              operation.abort();
              const schedule = transientReadFailure(error)
                ? retrySchedule(error, ++failures)
                : undefined;
              const retained = retainHistoryOnFailure(error, stateRef.current, agentId);
              if (retained !== undefined) {
                setState({
                  ...retained,
                  ...(schedule ? { historyRetryAt: Date.now() + schedule.next } : {}),
                });
                return;
              }
              actionAbortRef.current?.abort();
              replayAbortRef.current?.abort();
              hydration.current.reset();
              setState({
                status: 'error',
                message: errorMessage(error),
                ...(error instanceof WorkspaceReadUnavailable
                  ? { workspaceUnavailable: true }
                  : {}),
                ...(schedule ? { retryAt: Date.now() + schedule.next } : {}),
              });
              return schedule;
            }
          });
      },
      { initialDelay: Math.max(0, agentReadRetryAt(stateRef.current) - Date.now()) },
    );
    return () => observer.dispose();
  }, [
    agentId,
    client,
    lineage,
    publishHistory,
    revision,
    setActionError,
    setState,
    stateRef,
    suspended,
    metadataVersion,
  ]);

  const historyUnavailable =
    state.status === 'ready' && state.detail.id === agentId && state.historyUnavailable === true;
  const historyLoading =
    state.status === 'ready' && state.detail.id === agentId && state.historyLoading === true;
  const historyIncomplete = historyLoading || historyUnavailable;
  const controlReady = controlReadiness.controlBaselineReady(state, agentId, suspended);
  const v2 =
    !suspended && state.status === 'ready' && state.detail.id === agentId ? state.v2 : undefined;
  useHistoryRetry({
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
  });
  const replayEnabled =
    !suspended &&
    agentId !== undefined &&
    state.status === 'ready' &&
    state.detail.id === agentId &&
    (v2 === undefined
      ? state.replayReady === true || !historyIncomplete
      : state.conversation.agentCursor !== undefined) &&
    (v2 !== undefined || (pending !== 'send' && pending !== 'approval'));

  const provisioning =
    !suspended &&
    state.status === 'ready' &&
    state.detail.id === agentId &&
    !historyIncomplete &&
    (state.detail.status === 'starting' ||
      (state.detail.firstMessage !== undefined &&
        !['completed', 'failed', 'recovery_required'].includes(state.detail.firstMessage.state)));
  usePreparationProgress({
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
    wake: readinessWake,
  });
  useEffect(() => {
    if (!replayEnabled || agentId === undefined) return;
    const controller = new AbortController();
    replayAbortRef.current = controller;
    queueMicrotask(() => {
      if (!controller.signal.aborted) setStreamError(undefined);
    });
    const after =
      stateRef.current.status === 'ready'
        ? stateRef.current.conversation.cursor
        : liveCursor.current;
    hydration.current.replayFrom(after);
    const replay =
      v2 === undefined
        ? client.replayAgent(agentId, {
            after,
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
              if (
                frame.type === 'cma.agent.event' &&
                frame.data.kind?.type === 'conversation_available'
              )
                readinessWake.hint();
              const current = stateRef.current;
              if (current.status !== 'ready' || current.detail.id !== agentId)
                throw new Error('The Agent view no longer accepts live updates.');
              const conversation = hydration.current.applyV2(
                current.conversation,
                frame,
                agentId,
                current.historyLoading === true || current.historyUnavailable === true,
              );
              liveCursor.current = conversation.cursor;
              setState({ ...current, conversation });
            },
            setStreamError,
            observeApprovalProjection(v2, agentId, controller.signal, stateRef, setState),
          );
    void replay.catch((error: unknown) => {
      if (controller.signal.aborted || onIdentityChange(error)) return;
      setStreamError(`Live Agent updates are unavailable. ${errorMessage(error)}`);
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
    readinessWake,
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
  const applyAgentTitle = useAgentTitle(agentId, setState);
  const { run, command, recoverPending, initialSend } = useAgentActions({
    agentId: suspended ? undefined : agentId,
    client,
    historyIncomplete: historyIncomplete || suspended,
    commandsBlocked: !controlReady,
    activeTurn,
    responseTarget,
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
    refreshDetail,
    errorMessage,
    onIdentityChange,
    options,
  });
  const presented = usePresentedAgent(approvalProjectionView(state), summary);
  const legacy = client.agentTransport?.legacySession(
    suspended ? undefined : agentId,
    options.viewerId,
  );
  const retainedInputs = usePresentedRetainedInputs(
    v2,
    legacy,
    pending,
    onIdentityChange,
    recoverInputError,
  );
  const controlStatus = useControlStatus(v2, responseTarget, onIdentityChange);
  const responseControls = useFirstReplyStop(
    state,
    controlReadiness.responseControlAvailability(state, agentId, suspended),
  );
  const queuedInputs = useQueuedInputs(
    v2,
    controlReady,
    activeTurn,
    state.status === 'ready' ? queueObservationRevision(state.conversation) : '',
    onIdentityChange,
    requestRevision,
    state.status === 'ready' && state.detail.id === agentId
      ? state.conversation.messages
      : undefined,
  );
  return {
    retained,
    ...retainedInputs,
    queuedInputs: v2 === undefined ? undefined : queuedInputs,
    controlNotice: controlStatus.notice,
    stopRequest: controlStatus.stopRequest,
    controlBaselineReady: controlReady,
    responseControls,
    controlsUnavailable: controlReadiness.controlsUnavailable(state, v2 !== undefined, suspended),
    recoverPending: v2 === undefined && legacy === undefined ? undefined : recoverPending,
    state: presented,
    inputAvailability: accessCheckedInputAvailability(inputAvailability, suspended),
    resourceAvailability: suspended
      ? { state: 'unavailable', notice: 'Agent access is being checked. Live resources are held.' }
      : resourceAvailability,
    progressObservation,
    pending,
    initialSendPending: pending === 'send' && initialSend,
    actionError: actionError ?? streamError,
    streamError,
    reload,
    refreshDetail,
    applyAgentTitle,
    clearActionError: () => {
      setActionError(undefined);
      setStreamError(undefined);
    },
    send: (content, onAccepted, onReceiptObserved, onRefused, answerContext) =>
      run(content, onAccepted, onReceiptObserved, onRefused, 'send', undefined, answerContext),
    steer:
      v2 === undefined
        ? undefined
        : (content, onAccepted, onReceiptObserved, onRefused) =>
            run(content, onAccepted, onReceiptObserved, onRefused, 'steer'),
    interruptWithMessage:
      v2 === undefined
        ? undefined
        : (content, onAccepted, onReceiptObserved, onRefused, onOrdinaryInput) =>
            run(content, onAccepted, onReceiptObserved, onRefused, 'interrupt', onOrdinaryInput),
    approve: (approvalId, decision) => run({ id: approvalId, decision }),
    ...lifecycleCommands(client, command),
  };
}
