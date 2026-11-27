import type { AgentLiveEvent } from '../api/types';
import type { ConversationMessage, ConversationState } from './model';
import { upsert } from './conversationValues';

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

/** Replace a live row in place so its durable successor keeps its timeline slot. */
export function replaceLiveMessage(
  messages: readonly ConversationMessage[],
  runId: string | undefined,
  message: ConversationMessage,
) {
  const index = messages.findIndex((item) => item.liveOnly && item.runId === runId);
  if (index < 0) return { messages, order: undefined, replaced: false };
  const order = messages[index]?.order;
  return {
    messages: messages.map((item, current) =>
      current === index
        ? {
            ...message,
            renderKey: message.renderKey ?? item.renderKey ?? item.id,
            order: message.order ?? order,
          }
        : item,
    ),
    order,
    replaced: true,
  };
}

export function clearLiveGapRun(runs: ReadonlySet<string> | undefined, runId: string) {
  if (runs === undefined || !runs.has(runId)) return runs;
  const next = new Set(runs);
  next.delete(runId);
  return next.size === 0 ? undefined : next;
}

export function durableAssistantMessage(
  messages: readonly ConversationMessage[],
  runId: string | undefined,
  message: ConversationMessage,
) {
  const reconciled = replaceLiveMessage(messages, runId, message);
  return reconciled.replaced
    ? reconciled.messages
    : upsert(reconciled.messages, { ...message, order: reconciled.order });
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
        renderKey: current?.renderKey ?? id,
        order: current?.order ?? reconciled.order,
      },
    );
  }
  if (event.type === 'TEXT_MESSAGE_CONTENT') {
    if (typeof event.delta !== 'string' || event.delta === '') return state;
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
            renderKey: id,
          }
        : undefined);
    return message === undefined
      ? state
      : put(state, {
          ...message,
          at: message.at ?? event.timestamp,
          progressAt:
            event.delta.trim() === ''
              ? message.progressAt
              : (event.timestamp ?? message.progressAt),
          content: `${message.content}${event.delta}`,
        });
  }
  return current === undefined ? state : put(state, { ...current, status: 'complete' });
}
