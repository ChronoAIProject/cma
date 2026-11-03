import type { AgentResourceState } from './controllerTypes';
import type { AgentDetail, AgentStatus } from '../api/types';
import { mergeRuntimeMetadata } from './runtimeMetadata';

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
    (!setupPreview &&
      boundedTitle(
        metadata?.description,
        metadata?.descriptionSource === 'prompt_preview' ? 48 : 64,
      )) ||
    (configured !== agent.id && configured !== `Agent ${agent.id}` && configured) ||
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
export function presentAgent(state: AgentResourceState, status?: AgentStatus): AgentResourceState {
  if (state.status !== 'ready') return state;
  const ownerTitle = state.conversation.ownerTitle ?? state.detail.ownerTitle;
  const saved = state.detail.runtimeMetadata;
  const runtimeMetadata = mergeRuntimeMetadata(saved, state.conversation.runtimeMetadata);
  if (
    (status === undefined || status === state.detail.status) &&
    runtimeMetadata === saved &&
    ownerTitle === state.detail.ownerTitle
  )
    return state;
  return {
    ...state,
    detail: {
      ...state.detail,
      status: status ?? state.detail.status,
      runtimeMetadata,
      ownerTitle,
    },
  };
}
