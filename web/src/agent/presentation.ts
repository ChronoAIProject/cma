import type { AgentResourceState } from './controllerTypes';
import type { AgentStatus } from '../api/types';

/** Runtime notifications update visible metadata without a hierarchy refresh. */
export function presentAgent(state: AgentResourceState, status?: AgentStatus): AgentResourceState {
  if (state.status !== 'ready') return state;
  const saved = state.detail.runtimeMetadata;
  const live = state.conversation.runtimeMetadata;
  const workingDirectory = live?.workingDirectory ?? saved?.workingDirectory;
  const description = live?.description ?? saved?.description;
  if (
    (status === undefined || status === state.detail.status) &&
    workingDirectory === saved?.workingDirectory &&
    description === saved?.description
  )
    return state;
  return {
    ...state,
    detail: {
      ...state.detail,
      status: status ?? state.detail.status,
      runtimeMetadata: { workingDirectory, description },
    },
  };
}
