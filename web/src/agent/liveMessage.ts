import type { AgentLiveEvent } from '../api/types';
import type { ConversationMessage, ConversationState } from './model';

/** A durable message replaces the pending live block, never appends a second copy. */
export function consumeLiveMessage(
  messages: readonly ConversationMessage[],
  runId: string | undefined,
) {
  const live = messages.find((message) => message.liveOnly && message.runId === runId);
  return {
    messages: live === undefined ? messages : messages.filter((message) => message.id !== live.id),
    order: live?.order,
  };
}

function put(state: ConversationState, message: ConversationMessage): ConversationState {
  const found = state.messages.some((item) => item.id === message.id);
  return {
    ...state,
    messages: found
      ? state.messages.map((item) => (item.id === message.id ? message : item))
      : [...state.messages, message],
  };
}

export function liveMessage(state: ConversationState, event: AgentLiveEvent): ConversationState {
  if (typeof event.messageId !== 'string') return state;
  const id = event.messageId;
  const current = state.messages.find((message) => message.id === id);
  if (event.type === 'TEXT_MESSAGE_START') {
    const role = event.role === 'user' ? 'user' : 'assistant';
    const reconciled =
      role === 'assistant'
        ? consumeLiveMessage(state.messages, event.runId)
        : { messages: state.messages, order: undefined };
    return put(
      { ...state, messages: reconciled.messages },
      {
        id,
        runId: event.runId,
        role,
        content: '',
        status: 'streaming',
        at: event.timestamp,
        order: current?.order ?? reconciled.order,
      },
    );
  }
  if (event.type === 'TEXT_MESSAGE_CONTENT') {
    if (typeof event.delta !== 'string') return state;
    const message =
      current ??
      (event['cma:live'] === true
        ? {
            id,
            runId: event.runId,
            role: 'assistant' as const,
            content: '',
            status: 'streaming' as const,
            at: event.timestamp,
            liveOnly: true,
          }
        : undefined);
    return message === undefined
      ? state
      : put(state, {
          ...message,
          at: message.at ?? event.timestamp,
          content: `${message.content}${event.delta}`,
        });
  }
  return current === undefined ? state : put(state, { ...current, status: 'complete' });
}
