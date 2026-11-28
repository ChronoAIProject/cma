import type { AgentCommand, AgentResourceState } from './controllerTypes';
import { useEffect, useMemo, useState } from 'react';
import type { AgentDetail, AgentStatus, AgentSummary } from '../api/types';
import { observedActivity } from './activityStatus';
import { currentAgentStatus } from './status';
import { mergeRuntimeMetadata } from './runtimeMetadata';
import type { AgentV2Session } from './transport/client';

export function usePresentedAgent(state: AgentResourceState, summary?: AgentSummary) {
  return useMemo(
    () =>
      presentAgent(
        state,
        state.status === 'ready'
          ? currentAgentStatus(state.detail, state.conversation, summary)
          : undefined,
        summary,
      ),
    [state, summary],
  );
}

export function usePresentedRetainedInputs(
  v2: AgentV2Session | undefined,
  legacy: AgentV2Session | undefined,
  pending: AgentCommand | undefined,
  onIdentityChange: (error: unknown) => boolean,
  recoverInputError?: (
    session: AgentV2Session,
    key: string,
  ) => (outcome?: 'accepted' | 'refused') => void,
) {
  const [checked, setChecked] = useState<{ session: AgentV2Session; key: string }>();
  const legacyInput = legacy?.journal.legacySuggestion.read();
  const session = v2 ?? legacy;
  const key =
    pending !== undefined
      ? undefined
      : v2
        ? pendingInputKey(v2)
        : legacyInput?.state === 'pending'
          ? legacyInput.key
          : undefined;
  const checkingInputReceipt =
    session !== undefined &&
    key !== undefined &&
    (checked?.session !== session || checked.key !== key);
  useEffect(() => {
    if (!session || key === undefined || !checkingInputReceipt) return;
    const controller = new AbortController();
    const recovered = recoverInputError?.(session, key);
    const receipt =
      v2 === undefined
        ? session.recoverLegacySuggestion(controller.signal)
        : session.recover(() => {
            if (!controller.signal.aborted) recovered?.();
          }, controller.signal);
    void receipt
      .then((result) => {
        if (v2 && result === 'refused' && !controller.signal.aborted) recovered?.('refused');
      })
      .catch((error: unknown) => {
        if (!controller.signal.aborted) onIdentityChange(error);
      })
      .finally(() => {
        if (!controller.signal.aborted) setChecked({ session, key });
      });
    return () => controller.abort();
  }, [checkingInputReceipt, key, onIdentityChange, recoverInputError, session, v2]);

  const retained = v2?.pending();
  const inputCoolingDown = useInputCooldown(retained?.inputRetryAt);
  return {
    readSuggestionRequest: () => v2?.journal.read() ?? legacy?.journal.legacySuggestion.read(),
    pendingMessage:
      retained?.input ?? (legacyInput?.state === 'pending' ? legacyInput.input : undefined),
    acceptedMessage:
      retained?.accepted ?? (legacyInput?.state === 'accepted' ? legacyInput.input : undefined),
    acceptedSteeringMessage: retained?.acceptedSteering,
    acceptedInterruptionMessage: retained?.acceptedInterruption,
    pendingControl: retained?.control,
    checkingInputReceipt,
    inputCoolingDown,
  };
}

function useInputCooldown(retryAt: number | undefined) {
  const [elapsed, setElapsed] = useState<number>();
  useEffect(() => {
    if (retryAt === undefined || retryAt === elapsed) return;
    const wake = () => {
      const remaining = retryAt - Date.now();
      if (remaining <= 0) setElapsed(retryAt);
      else timer = setTimeout(wake, remaining);
    };
    let timer = setTimeout(wake, Math.max(0, retryAt - Date.now()));
    return () => clearTimeout(timer);
  }, [retryAt, elapsed]);
  return retryAt !== undefined && retryAt !== elapsed;
}

function pendingInputKey(session: AgentV2Session | undefined) {
  try {
    const input = session?.journal.read();
    return input?.state === 'pending' ? input.key : undefined;
  } catch {
    return undefined;
  }
}

/** The same label is used by the header, document title and hierarchy. */
export function agentDisplayTitle(
  agent: Pick<AgentDetail, 'title' | 'ownerTitle' | 'runtimeMetadata'> & { readonly id?: string },
): string {
  const metadata = agent.runtimeMetadata;
  const setupPreview =
    metadata?.descriptionSource === 'prompt_preview' && metadata.descriptionIsSetupPreview === true;
  const configured = boundedTitle(agent.title);
  return (
    agent.ownerTitle?.trim() ||
    (configured !== agent.id && configured !== `Agent ${agent.id}` && configured) ||
    (!setupPreview &&
      boundedTitle(
        metadata?.description,
        metadata?.descriptionSource === 'prompt_preview' ? 48 : 64,
      )) ||
    'Untitled task'
  );
}

function boundedTitle(value: string | undefined, limit = 64): string | undefined {
  return value === undefined
    ? undefined
    : Array.from(value.trim().split(/[\r\n]/, 1)[0] ?? '')
        .filter((character) => !/\p{Cc}/u.test(character))
        .slice(0, limit)
        .join('')
        .trim();
}

/** Runtime notifications update visible metadata without a hierarchy refresh. */
export function presentAgent(
  state: AgentResourceState,
  status?: AgentStatus,
  summary?: AgentSummary,
): AgentResourceState {
  if (state.status !== 'ready') return state;
  const activityStatus = observedActivity(state, summary);
  const ownerTitle = state.conversation.ownerTitle ?? state.detail.ownerTitle;
  const title = state.conversation.title ?? state.detail.title;
  const saved = state.detail.runtimeMetadata;
  const runtimeMetadata = mergeRuntimeMetadata(saved, state.conversation.runtimeMetadata);
  if (
    activityStatus === state.activityStatus &&
    (status === undefined || status === state.detail.status) &&
    runtimeMetadata === saved &&
    ownerTitle === state.detail.ownerTitle &&
    title === state.detail.title
  )
    return state;
  return {
    ...state,
    activityStatus,
    detail: {
      ...state.detail,
      status: status ?? state.detail.status,
      runtimeMetadata,
      ownerTitle,
      title,
    },
  };
}
