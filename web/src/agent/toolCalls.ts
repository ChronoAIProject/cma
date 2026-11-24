import type { AgentLiveEvent } from '../api/types';
import type { ConversationState, ToolCall } from './model';
import { isPendingTool } from './toolStatus';
import { object } from './conversationValues';

function text(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

/** Replayed starts and intermediate updates cannot turn a settled call back into pending work. */
export function updateTool(
  state: ConversationState,
  item: ToolCall,
  start = false,
): ConversationState {
  const current = state.toolCalls.find((tool) => tool.id === item.id);
  const tool =
    start && current !== undefined
      ? current.startedAt || !item.startedAt
        ? current
        : { ...current, startedAt: item.startedAt }
      : {
          ...current,
          ...item,
          title: item.title.trim() ? item.title : (current?.title ?? item.title),
          status:
            current !== undefined && !isPendingTool(current) && isPendingTool(item)
              ? current.status
              : item.status,
        };
  return {
    ...state,
    toolCalls:
      current === undefined
        ? [...state.toolCalls, tool]
        : state.toolCalls.map((entry) => (entry.id === tool.id ? tool : entry)),
  };
}

export function liveTool(state: ConversationState, event: AgentLiveEvent): ConversationState {
  const id = text(event.toolCallId);
  if (id === undefined) return state;
  const current = state.toolCalls.find((tool) => tool.id === id);
  if (event.type === 'CUSTOM' && current === undefined) return state;
  if (event.type === 'TOOL_CALL_START') {
    return updateTool(
      state,
      {
        id,
        runId: event.runId,
        title: text(event.toolCallName) ?? 'Tool call',
        kind: text(event['cma:toolKind']),
        status: text(event['cma:status']) ?? 'running',
        startedAt: event.timestamp,
      },
      true,
    );
  }
  const update = text(event.delta);
  const output = object(event['cma:output']);
  const detail =
    event.type === 'TOOL_CALL_ARGS' && update !== undefined
      ? `${current?.detail ?? ''}${update}`
      : (text(event.result) ?? update ?? current?.detail);
  return updateTool(state, {
    id,
    runId: event.runId,
    title: text(event.toolCallName) ?? current?.title ?? 'Tool call',
    ...current,
    status:
      text(event['cma:status']) ??
      (event.type === 'TOOL_CALL_END' ? 'completed' : (current?.status ?? 'running')),
    detail,
    finishedAt:
      current?.finishedAt ?? (event.type === 'TOOL_CALL_END' ? event.timestamp : undefined),
    output:
      typeof output?.text === 'string' && typeof output.truncated === 'boolean'
        ? { text: output.text, truncated: output.truncated }
        : current?.output,
  });
}
