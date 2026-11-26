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
        startedAt: event.timestamp,
      }),
    };
  }
  if (event.type.includes('CONTENT')) {
    const block = current ?? { id, runId: event.runId, content: '', status: 'streaming' as const };
    const delta = text(event.delta) ?? text(event.content) ?? '';
    if (delta === '') return state;
    return {
      ...state,
      reasoning: upsert(state.reasoning, {
        ...block,
        startedAt: current?.startedAt ?? event.timestamp,
        progressAt:
          delta.trim() === '' ? current?.progressAt : (event.timestamp ?? current?.progressAt),
        content: `${block.content}${delta}`,
      }),
    };
  }
  return current === undefined
    ? state
    : { ...state, reasoning: upsert(state.reasoning, { ...current, status: 'complete' }) };
}
