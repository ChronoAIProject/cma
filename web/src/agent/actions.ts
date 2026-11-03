import {
  useCallback,
  useLayoutEffect,
  useMemo,
  useRef,
  type Dispatch,
  type SetStateAction,
  type RefObject,
} from 'react';
import type { CmaWebClient, CommandKeys } from '../api/client';
import type { AgentLiveEvent } from '../api/types';
import type { AgentCommand, AgentResourceState, ApprovalDecision } from './controller';
import type { AgentV2Session } from './transport/client';
import type { MessageAcceptance } from './controllerTypes';
import { ApiError, type AnswerContext } from '@chronoai/cma-sdk';
import { submitV2Control, recoverV2Control } from './transport/controls';
import {
  submitV2Interruption,
  recoverV2Interruption,
  InterruptionNotSubmittedError,
} from './transport/interruption';

interface Options {
  readonly agentId?: string;
  readonly client: CmaWebClient;
  readonly historyIncomplete: boolean;
  readonly activeTurn: boolean;
  readonly inputAvailability: { readonly allowed: boolean };
  readonly actionAbortRef: RefObject<AbortController | undefined>;
  readonly actionKindRef: RefObject<AgentCommand | undefined>;
  readonly replayAbortRef: RefObject<AbortController | undefined>;
  readonly stateRef: RefObject<AgentResourceState>;
  readonly keys: CommandKeys;
  readonly v2?: AgentV2Session;
  readonly setPending: Dispatch<SetStateAction<AgentCommand | undefined>>;
  readonly setActionError: Dispatch<SetStateAction<string | undefined>>;
  readonly setReplayRevision: Dispatch<SetStateAction<number>>;
  readonly setRequestRevision: Dispatch<SetStateAction<number>>;
  readonly applyLive: (event: AgentLiveEvent) => void;
  readonly reload: () => void;
  readonly errorMessage: (error: unknown) => string;
  readonly onIdentityChange: (error: unknown) => boolean;
  readonly options: {
    readonly viewerId?: string;
    readonly onHierarchyChange?: () => void;
    readonly onDeleted?: () => void;
  };
}
function runId(): string {
  return typeof crypto.randomUUID === 'function'
    ? crypto.randomUUID()
    : `run-${String(Date.now())}-${String(Math.random()).slice(2)}`;
}
export function useAgentActions({
  agentId,
  client,
  historyIncomplete,
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
}: Options) {
  const context = useMemo(
    () => ({
      agentId,
      client,
      viewerId: options.viewerId,
      allowed: inputAvailability.allowed,
    }),
    [agentId, client, options.viewerId, inputAvailability.allowed],
  );
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
      recovering = false,
    ) => {
      if (
        currentContext.current !== context ||
        agentId === undefined ||
        actionAbortRef.current !== undefined ||
        (historyIncomplete && typeof approval !== 'string') ||
        !inputAvailability.allowed
      )
        return;
      const current = stateRef.current;
      if (current.status !== 'ready' || current.detail.id !== agentId) return;
      if (answerContext && !recovering) {
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
      )
        return;
      if (session === undefined) replayAbortRef.current?.abort();
      const controller = new AbortController();
      actionAbortRef.current = controller;
      const approving = typeof approval !== 'string';
      actionKindRef.current = approving ? 'approval' : 'send';
      setPending(approving ? 'approval' : 'send');
      setActionError(undefined);
      let accepted = false;
      let legacyKey: string | undefined;
      try {
        if (session !== undefined) {
          const sendInput = (text: string) =>
            session.send(
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
            );
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
          legacyKey = legacy.journal.legacySuggestion.retain(approval, answerContext).key;
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
          error instanceof ApiError &&
          ((error.status === 422 &&
            ['invalid_run_request', 'invalid_answer_context'].includes(error.code ?? '')) ||
            (error.status === 409 && error.code === 'agent_not_ready'))
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
          (error instanceof InterruptionNotSubmittedError ||
            (error instanceof ApiError &&
              ((error.status === 422 &&
                [
                  'invalid_response_input',
                  'invalid_answer_context',
                  'invalid_control_request',
                  'invalid_interrupt_request',
                  ...(mode === 'interrupt' ? ['capability_unsupported'] : []),
                ].includes(error.code ?? '')) ||
                (error.status === 409 &&
                  [
                    'response_control_not_admitted',
                    'response_interrupt_not_admitted',
                    'agent_command_not_admissible',
                  ].includes(error.code ?? '')))))
        )
          onRefused?.(
            answerContext &&
              error instanceof ApiError &&
              error.code === 'agent_command_not_admissible'
              ? 'answer_changed'
              : undefined,
          );
        if (!controller.signal.aborted && !onIdentityChange(error))
          setActionError(errorMessage(error));
      } finally {
        if (actionAbortRef.current === controller) {
          actionAbortRef.current = undefined;
          actionKindRef.current = undefined;
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
    ) => {
      const mayPreemptSend = name === 'interrupt' && actionKindRef.current === 'send';
      if (currentContext.current !== context || historyIncomplete) return;
      if (name === 'interrupt' && !activeTurn) return;
      if (agentId === undefined || (actionAbortRef.current !== undefined && !mayPreemptSend)) {
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
        else if (name !== 'interrupt' || v2 === undefined) reload();
      } catch (error: unknown) {
        if (!controller.signal.aborted && !onIdentityChange(error))
          setActionError(errorMessage(error));
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
      historyIncomplete,
      keys,
      options,
      reload,
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
    if (v2 === undefined) {
      const legacy = client.agentTransport?.legacySession(agentId, options.viewerId);
      const pending = legacy?.journal.legacySuggestion.read();
      if (pending?.state === 'pending')
        await run(
          pending.input,
          undefined,
          undefined,
          undefined,
          'send',
          undefined,
          pending.answerContext,
          true,
        );
      return;
    }
    if (v2 === undefined || actionAbortRef.current !== undefined || historyIncomplete) return;
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
    setPending('send');
    setActionError(undefined);
    try {
      await v2.recover(() => {}, controller.signal);
      await recoverV2Control(v2, controller.signal);
      await recoverV2Interruption(v2, controller.signal);
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

  return { run, command, recoverPending };
}
