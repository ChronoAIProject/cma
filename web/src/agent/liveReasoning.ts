import type { AgentLiveEvent } from '../api/types';
import type { ConversationState } from './model';
import { text, upsert } from './conversationValues';

export function liveReasoning(state: ConversationState, event: AgentLiveEvent): ConversationState {
  const id = text(event.messageId) ?? `reasoning:${event.runId}`;
  const current = state.reasoning.find((reasoning) => reasoning.id === id);
  if (event.type.endsWith('START')) {
    return {
      ...state,
      reasoning: upsert(state.reasoning, {
        id,
        runId: event.runId,
        content: '',
        status: 'streaming',
      }),
    };
  }
  if (event.type.includes('CONTENT')) {
    const block = current ?? { id, runId: event.runId, content: '', status: 'streaming' as const };
    return {
      ...state,
      reasoning: upsert(state.reasoning, {
        ...block,
        content: `${block.content}${text(event.delta) ?? text(event.content) ?? ''}`,
      }),
    };
  }
  return current === undefined
    ? state
    : { ...state, reasoning: upsert(state.reasoning, { ...current, status: 'complete' }) };
}
