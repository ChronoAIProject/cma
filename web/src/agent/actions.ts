import { useCallback, useLayoutEffect, useMemo, useRef, useState } from 'react';
import type { AgentActionOptions } from './actionOptions';
import type { InputErrorOwner } from './actionError';
import type { AgentCommand, ApprovalDecision, MessageAcceptance } from './controllerTypes';
import { ApiError, type AnswerContext } from '@cma/client';
import { DraftBindingError } from './draftDelivery';
import {
  isLegacySuggestionRefusal,
  isV2InputRefusal,
  stoppedInputMessage,
} from './controllerError';
import { agentRestartReady } from './controlReadiness';
import { runId } from './transport/runId';
import { submitV2Control, recoverV2Control } from './transport/controls';
import {
  submitV2Interruption,
  recoverV2Interruption,
  InterruptionNotSubmittedError,
} from './transport/interruption';
export function useAgentActions({
  agentId,
  client,
  historyIncomplete,
  commandsBlocked,
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
  refreshDetail,
  errorMessage,
  onIdentityChange,
  options,
}: AgentActionOptions) {
  const [initialSend, setInitialSend] = useState(false);
  const context = useMemo(
    () => ({
      agentId,
      client,
      viewerId: options.viewerId,
      allowed: inputAvailability.allowed,
    }),
    [agentId, client, options.viewerId, inputAvailability.allowed],
  );
  // Revoking controls must not invalidate an ordinary send's in-flight receipt.
  const currentCommandsBlocked = useRef(commandsBlocked);
  useLayoutEffect(() => {
    currentCommandsBlocked.current = commandsBlocked;
  }, [commandsBlocked]);
  const currentContext = useRef<typeof context | undefined>(undefined);
  useLayoutEffect(() => {
    currentContext.current = context;
    return () => {
      currentContext.current = undefined;
    };
  }, [context]);
  const run = useCallback(
    async (
      approval: { readonly id: string; readonly decision: ApprovalDecision } | string,
      onAccepted?: (receipt?: MessageAcceptance) => void,
      onReceiptObserved?: (receipt: MessageAcceptance) => void,
      onRefused?: (reason?: 'answer_changed' | 'suggestions_unavailable') => void,
      mode: 'send' | 'steer' | 'interrupt' = 'send',
      onOrdinaryInput?: () => void,
      answerContext?: AnswerContext,
    ) => {
      if (
        currentContext.current !== context ||
        agentId === undefined ||
        actionAbortRef.current !== undefined ||
        ((commandsBlocked || currentCommandsBlocked.current) && typeof approval !== 'string') ||
        !inputAvailability.allowed
      ) {
        if (typeof approval === 'string' && mode === 'send') onRefused?.();
        return;
      }
      const current = stateRef.current;
      if (current.status !== 'ready' || current.detail.id !== agentId) {
        if (typeof approval === 'string' && mode === 'send') onRefused?.();
        return;
      }
      if (
        historyIncomplete &&
        typeof approval !== 'string' &&
        !current.conversation.approvals.some((request) => request.id === approval.id)
      )
        return;
      if (answerContext) {
        const answer = current.conversation.messages.at(-1);
        if (
          current.conversation.activeRunId ||
          !answer?.finalAnswer ||
          answer.sequence !== answerContext.answer_sequence ||
          answer.runId !== String(answerContext.turn_sequence) ||
          current.conversation.latestTurnId !== answer.runId
        ) {
          onRefused?.('answer_changed');
          setActionError('This answer has changed. Use the latest answer or the message composer.');
          return;
        }
      }
      const session = current.status === 'ready' ? current.v2 : undefined;
      const legacy =
        session === undefined
          ? client.agentTransport?.legacySession(agentId, options.viewerId)
          : undefined;
      if (
        current.detail.status === 'starting' &&
        (session === undefined || typeof approval !== 'string')
      ) {
        if (typeof approval === 'string' && mode === 'send') onRefused?.();
        return;
      }
      if (session === undefined) replayAbortRef.current?.abort();
      const controller = new AbortController();
      actionAbortRef.current = controller;
      const approving = typeof approval !== 'string';
      actionKindRef.current = approving ? 'approval' : 'send';
      setInitialSend(!approving && mode === 'send');
      setPending(approving ? 'approval' : 'send');
      setActionError(undefined);
      let accepted = false;
      let recoveringInput = false;
      let legacyKey: string | undefined;
      let retainedInput: InputErrorOwner | undefined;
      try {
        if (session !== undefined) {
          const sendInput = (text: string) => {
            recoveringInput = session.journal.read()?.state === 'pending';
            setInitialSend(!recoveringInput);
            return session.send(
              text,
              (receipt) => {
                if (controller.signal.aborted || currentContext.current !== context) return;
                accepted = true;
                onAccepted?.(receipt);
              },
              controller.signal,
              (receipt) => {
                if (!controller.signal.aborted && currentContext.current === context)
                  onReceiptObserved?.(receipt);
              },
              answerContext,
              (key) => {
                retainedInput = { session, key };
              },
            );
          };
          if (approving) {
            const scope = `agent:${agentId}:approval:${approval.id}:${approval.decision}`;
            await submitV2Control(
              session,
              current.status === 'ready' ? current.conversation.responseBinding : undefined,
              { type: 'resolve_approval', approval_id: approval.id, decision: approval.decision },
              runId(),
              controller.signal,
            );
            keys.settle(scope);
          } else if (mode === 'interrupt') {
            const result = await submitV2Interruption(
              session,
              current.conversation.responseBinding,
              approval,
              runId(),
              controller.signal,
            );
            if (result === undefined) {
              if (controller.signal.aborted || currentContext.current !== context) return;
              onOrdinaryInput?.();
              await sendInput(approval);
              return;
            }
            if (!controller.signal.aborted && currentContext.current === context) {
              const receipt = {
                commandId: result.response.receipt.command_id,
                responseId: result.response.receipt.response_id!,
              };
              accepted = true;
              onReceiptObserved?.(receipt);
              onAccepted?.(receipt);
            }
          } else if (mode === 'steer' && current.conversation.responseBinding !== undefined) {
            const result = await submitV2Control(
              session,
              current.conversation.responseBinding,
              { type: 'steer', text: approval },
              runId(),
              controller.signal,
            );
            if (!controller.signal.aborted && currentContext.current === context) {
              const receipt = {
                commandId: result.control.receipt.command_id,
                responseId: result.target.response_id,
              };
              accepted = true;
              onReceiptObserved?.(receipt);
              onAccepted?.(receipt);
            }
          } else await sendInput(approval);
          return;
        }
        if (answerContext !== undefined && !approving) {
          if (!legacy)
            throw new Error('Suggestion recovery is unavailable. Use the message composer.');
          await legacy.authorize(controller.signal);
          const previous = legacy.journal.legacySuggestion.read();
          const retained = legacy.journal.legacySuggestion.retain(approval, answerContext);
          if (previous?.state === 'pending') {
            setInitialSend(false);
            const runId = await legacy.recoverLegacySuggestion(controller.signal);
            if (!runId) throw new Error('The original suggestion is still awaiting confirmation.');
            if (!controller.signal.aborted && currentContext.current === context)
              onAccepted?.({ runId });
            return;
          }
          legacyKey = retained.key;
        } else if (legacy?.journal.legacySuggestion.read()?.state === 'pending') {
          throw new Error('Recover your pending suggestion before sending another message.');
        }
        await client.runAgent(
          {
            threadId: agentId,
            runId: legacyKey ?? runId(),
            ...(answerContext ? { forwardedProps: { answerContext } } : {}),
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
            ...(legacyKey ? { idempotencyKey: legacyKey } : {}),
            onEvent: (event) => {
              const apply = () => {
                if (controller.signal.aborted || event.threadId !== agentId) return;
                if (!accepted && event.type !== 'RUN_ERROR') {
                  if (legacyKey) legacy?.journal.legacySuggestion.accepted(legacyKey, event.runId);
                  accepted = true;
                  onAccepted?.({ runId: event.runId });
                }
                applyLive(event);
              };
              if (legacyKey) return legacy?.authorize(controller.signal).then(apply);
              apply();
            },
          },
        );
      } catch (error: unknown) {
        if (
          !controller.signal.aborted &&
          currentContext.current === context &&
          session !== undefined &&
          answerContext &&
          !accepted &&
          !recoveringInput &&
          error instanceof ApiError &&
          error.status === 422 &&
          error.code === 'responses_field_unsupported'
        ) {
          onRefused?.('suggestions_unavailable');
          setActionError(
            'Suggested replies are unavailable on this server. Continue in the message composer.',
          );
          return;
        }
        if (
          !controller.signal.aborted &&
          currentContext.current === context &&
          legacyKey &&
          !accepted &&
          isLegacySuggestionRefusal(error)
        ) {
          legacy?.journal.legacySuggestion.refused(legacyKey);
          onRefused?.(error.status === 409 ? 'answer_changed' : undefined);
        }
        if (
          !controller.signal.aborted &&
          currentContext.current === context &&
          session !== undefined &&
          !approving &&
          !accepted &&
          !recoveringInput &&
          (error instanceof DraftBindingError ||
            error instanceof InterruptionNotSubmittedError ||
            isV2InputRefusal(error, mode === 'interrupt'))
        )
          onRefused?.(
            answerContext &&
              error instanceof ApiError &&
              error.code === 'agent_command_not_admissible'
              ? 'answer_changed'
              : undefined,
          );
        if (!controller.signal.aborted && !onIdentityChange(error))
          setActionError(
            (!approving && !accepted && stoppedInputMessage(error, stateRef.current, agentId)) ||
              errorMessage(error),
            retainedInput,
          );
      } finally {
        if (actionAbortRef.current === controller) {
          actionAbortRef.current = undefined;
          actionKindRef.current = undefined;
          setInitialSend(false);
          setPending(undefined);
          setRequestRevision((value) => value + 1);
          if (session === undefined) setReplayRevision((value) => value + 1);
        }
      }
    },
    [
      agentId,
      applyLive,
      client,
      context,
      historyIncomplete,
      commandsBlocked,
      inputAvailability.allowed,
      keys,
      stateRef,
      actionAbortRef,
      actionKindRef,
      errorMessage,
      onIdentityChange,
      options.viewerId,
      replayAbortRef,
      setActionError,
      setPending,
      setReplayRevision,
      setRequestRevision,
    ],
  );

  const command = useCallback(
    async (
      name: Exclude<AgentCommand, 'send' | 'approval'>,
      operation: (id: string, key: string, signal: AbortSignal) => Promise<void>,
      requireStop = false,
    ) => {
      const mayPreemptSend = name === 'interrupt' && actionKindRef.current === 'send';
      const ready =
        name === 'restart'
          ? agentRestartReady(stateRef.current, agentId)
          : !commandsBlocked && !currentCommandsBlocked.current;
      if (currentContext.current !== context || !ready) {
        if (requireStop)
          throw new Error('Agent activity is still loading. Try again when it is available.');
        return;
      }
      if (name === 'interrupt' && !activeTurn) return;
      if (agentId === undefined || (actionAbortRef.current !== undefined && !mayPreemptSend)) {
        if (requireStop) throw new Error('Another Agent action is still being confirmed.');
        return;
      }
      if (name === 'interrupt') actionAbortRef.current?.abort();
      const scope = `agent:${agentId}:${name}`;
      const controller = new AbortController();
      actionAbortRef.current = controller;
      actionKindRef.current = name;
      setPending(name);
      setActionError(undefined);
      try {
        const current = stateRef.current;
        if (name === 'interrupt' && current.status === 'ready' && current.v2 !== undefined)
          await submitV2Control(
            current.v2,
            current.conversation.responseBinding,
            { type: 'cancel' },
            runId(),
            controller.signal,
          );
        else {
          const session = current.status === 'ready' ? current.v2 : undefined;
          await session?.authorize(controller.signal);
          await operation(agentId, keys.for(scope), controller.signal);
          await session?.authorize(controller.signal);
        }
        keys.settle(scope);
        options.onHierarchyChange?.();
        if (name === 'delete') options.onDeleted?.();
        else if (name === 'stop' && v2 !== undefined) refreshDetail();
        else if (name !== 'interrupt' || (v2 === undefined && !requireStop)) reload();
      } catch (error: unknown) {
        if (!controller.signal.aborted && !onIdentityChange(error))
          setActionError(errorMessage(error));
        if (requireStop) throw error;
      } finally {
        if (actionAbortRef.current === controller) {
          actionAbortRef.current = undefined;
          actionKindRef.current = undefined;
          setPending(undefined);
          setRequestRevision((value) => value + 1);
          if (name === 'interrupt' && v2 === undefined) setReplayRevision((value) => value + 1);
        }
      }
    },
    [
      activeTurn,
      context,
      agentId,
      commandsBlocked,
      keys,
      options,
      reload,
      refreshDetail,
      stateRef,
      v2,
      actionAbortRef,
      actionKindRef,
      errorMessage,
      onIdentityChange,
      setActionError,
      setPending,
      setReplayRevision,
      setRequestRevision,
    ],
  );

  const recoverPending = async () => {
    if (currentContext.current !== context) return;
    const legacy =
      v2 === undefined
        ? client.agentTransport?.legacySession(agentId, options.viewerId)
        : undefined;
    if (
      (v2 === undefined && legacy === undefined) ||
      actionAbortRef.current !== undefined ||
      historyIncomplete
    )
      return;
    const current = stateRef.current;
    if (
      current.status === 'ready' &&
      current.detail.status === 'starting' &&
      !inputAvailability.allowed
    )
      return;
    const controller = new AbortController();
    actionAbortRef.current = controller;
    actionKindRef.current = 'send';
    setInitialSend(false);
    setPending('send');
    setActionError(undefined);
    try {
      if (v2 === undefined) {
        if (
          legacy?.journal.legacySuggestion.read()?.state === 'pending' &&
          !(await legacy.recoverLegacySuggestion(controller.signal))
        ) {
          setActionError('The original suggestion is still awaiting confirmation.');
        }
        return;
      }
      // Empty recovery helpers still authorize. Avoid making a retained Cancel
      // wait for unrelated input/interruption checks that have nothing to send.
      if (v2.journal.readControl()?.commandId === null)
        await recoverV2Control(v2, controller.signal);
      if (v2.journal.interruption.read()?.receipt === null)
        await recoverV2Interruption(v2, controller.signal);
      if (
        v2.journal.read()?.state === 'pending' &&
        !(await v2.recover(() => {}, controller.signal))
      ) {
        setActionError(
          'The original message is still awaiting confirmation. Check the pending request again.',
        );
      }
    } catch (error) {
      if (!controller.signal.aborted && !onIdentityChange(error))
        setActionError(errorMessage(error));
    } finally {
      if (actionAbortRef.current === controller) {
        actionAbortRef.current = undefined;
        actionKindRef.current = undefined;
        setPending(undefined);
        setRequestRevision((value) => value + 1);
      }
    }
  };

  return { run, command, recoverPending, initialSend };
}
