import type { AgentResourceState } from './controllerTypes';
import { useMemo } from 'react';
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

export function presentRetainedInputs(v2?: AgentV2Session, legacy?: AgentV2Session) {
  const retained = v2?.pending();
  const legacyInput = legacy?.journal.legacySuggestion.read();
  return {
    readSuggestionRequest: () => v2?.journal.read() ?? legacy?.journal.legacySuggestion.read(),
    pendingMessage:
      retained?.input ?? (legacyInput?.state === 'pending' ? legacyInput.input : undefined),
    acceptedMessage:
      retained?.accepted ?? (legacyInput?.state === 'accepted' ? legacyInput.input : undefined),
    acceptedSteeringMessage: retained?.acceptedSteering,
    acceptedInterruptionMessage: retained?.acceptedInterruption,
    pendingControl: retained?.control,
  };
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
