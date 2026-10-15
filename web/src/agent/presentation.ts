import type { AgentResourceState } from './controllerTypes';
import type { AgentDetail, AgentStatus } from '../api/types';
import { mergeRuntimeMetadata } from './runtimeMetadata';

/** The same label is used by the header, document title and hierarchy. */
export function agentDisplayTitle(agent: Pick<AgentDetail, 'title' | 'runtimeMetadata'>): string {
  const metadata = agent.runtimeMetadata;
  const setupPreview =
    metadata?.descriptionSource === 'prompt_preview' && metadata.descriptionIsSetupPreview === true;
  return (
    (!setupPreview && metadata?.description?.trim()) || agent.title?.trim() || 'Untitled agent'
  );
}

/** Runtime notifications update visible metadata without a hierarchy refresh. */
export function presentAgent(state: AgentResourceState, status?: AgentStatus): AgentResourceState {
  if (state.status !== 'ready') return state;
  const saved = state.detail.runtimeMetadata;
  const runtimeMetadata = mergeRuntimeMetadata(saved, state.conversation.runtimeMetadata);
  if ((status === undefined || status === state.detail.status) && runtimeMetadata === saved)
    return state;
  return {
    ...state,
    detail: {
      ...state.detail,
      status: status ?? state.detail.status,
      runtimeMetadata,
    },
  };
}
