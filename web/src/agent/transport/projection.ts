import type { V2AgentEventItem, V2AgentEventKind, V2AgentStreamFrame } from '@cma/client';
import { foldAgentEvent, foldLiveEvent } from '../conversation';
import type { ConversationState } from '../model';
import { foldExternalApproval } from './externalApprovalProjection';
import { settleLiveRecovery } from '../liveRecovery';

/** Adapt the safe common vocabulary to the existing durable conversation reducer. */
function normalized(kind: V2AgentEventKind) {
  switch (kind.type) {
    case 'message':
      return {
        type: kind.role === 'user' ? 'owner_message' : 'agent_message',
        content: kind.content,
      };
    case 'conversation_available':
      return { type: 'conversation_opened' };
    case 'tool_started':
      return { ...kind, type: 'tool_call_started' };
    case 'tool_updated':
      return { ...kind, type: 'tool_call_updated' };
    case 'turn_cancelled':
      return { ...kind, type: 'turn_interrupted' };
    case 'authentication_required':
      return { type: 'sign_in_required' };
    default:
      return kind;
  }
}

export function foldV2Item(state: ConversationState, item: V2AgentEventItem): ConversationState {
  if (item.sequence <= state.cursor) return state;
  if (
    item.kind?.type === 'external_approval_requested' ||
    item.kind?.type === 'external_approval_resolved'
  ) {
    const next = foldExternalApproval(state, item.kind, item.sequence);
    return { ...next, cursor: item.sequence, agentCursor: item.cursor };
  }
  let next =
    item.kind === null
      ? { ...state, cursor: item.sequence }
      : foldAgentEvent(state, {
          seq: item.sequence,
          appendedAt: item.appended_at,
          retainedOutput: item.retained_output,
          kind: normalized(item.kind),
        });
  if (item.kind?.type === 'turn_started')
    next = {
      ...next,
      responseBinding:
        item.response_binding === null
          ? undefined
          : {
              responseId: item.response_binding.response_id,
              commandId: item.response_binding.command_id,
              turnSequence: item.response_binding.turn_sequence,
            },
    };
  if (item.kind?.type === 'message' && item.kind.role === 'user') {
    // The original response owns only its first input. Steering within that same
    // turn requires its own server-proven command identity, never a text match.
    const firstInput = !state.messages.some(
      (message) => message.role === 'user' && message.runId === state.activeRunId,
    );
    const commandId =
      item.message_command_id ?? (firstInput ? state.responseBinding?.commandId : undefined);
    const responseId =
      state.responseBinding?.commandId === commandId
        ? state.responseBinding?.responseId
        : undefined;
    next = {
      ...next,
      messages: next.messages.map((message) =>
        message.id === `event:${item.sequence}` ? { ...message, commandId, responseId } : message,
      ),
    };
  }
  if (next.activeRunId === undefined) next = { ...next, responseBinding: undefined };
  if (item.kind?.type === 'stopped' || item.kind?.type === 'deleted')
    next = settleLiveRecovery({
      ...next,
      agentStatus: item.kind.type,
      agentStatusAt: item.appended_at,
      liveGapRuns: undefined,
      activeRunId: undefined,
      activeRunStartedAt: undefined,
      responseBinding: undefined,
      reasoning: [],
    });
  if (item.kind?.type === 'history_gap')
    next = gap(
      next,
      `history-gap:${item.sequence}`,
      'Some conversation updates were lost before they could be recorded.',
    );
  else if (item.kind?.type === 'turn_started')
    next = { ...next, recovery: next.recovery.filter((notice) => notice.clearsOn === undefined) };
  else if (
    item.kind?.type === 'turn_failed' ||
    item.kind?.type === 'turn_completed' ||
    item.kind?.type === 'turn_cancelled'
  )
    next = settleLiveRecovery(next);
  else if (item.kind?.type === 'message' && item.kind.role === 'assistant')
    next = clearLiveRecovery(next, state.activeRunId, true);
  return { ...next, agentCursor: item.cursor };
}

function clearLiveRecovery(
  state: ConversationState,
  runId: string | undefined,
  recorded: boolean,
): ConversationState {
  return {
    ...state,
    recovery: state.recovery.filter(
      (notice) =>
        notice.clearsOn === undefined ||
        (notice.runId !== undefined && notice.runId !== runId) ||
        (!recorded && notice.clearsOn === 'recorded-turn'),
    ),
  };
}

function gap(
  state: ConversationState,
  id: string,
  detail: string,
  live = false,
): ConversationState {
  const liveGapRuns =
    live && state.activeRunId !== undefined
      ? new Set([...(state.liveGapRuns ?? []), state.activeRunId])
      : state.liveGapRuns;
  return {
    ...state,
    liveGapRuns,
    recovery: [
      ...state.recovery.filter(
        (notice) =>
          notice.id !== id &&
          (!live || notice.clearsOn === undefined || notice.runId !== state.activeRunId),
      ),
      {
        id,
        runId: state.activeRunId,
        afterSequence: state.cursor,
        title: live
          ? state.activeRunId === undefined
            ? 'Live updates interrupted'
            : 'Refreshing live updates…'
          : 'Incomplete conversation history',
        detail,
        clearsOn: live
          ? state.messages.some((message) => message.liveOnly) ||
            state.recovery.some(
              (notice) => notice.clearsOn === 'recorded-turn' && notice.runId === state.activeRunId,
            )
            ? 'recorded-turn'
            : 'live-update'
          : undefined,
      },
    ],
  };
}

export function foldV2Frame(
  state: ConversationState,
  frame: V2AgentStreamFrame,
  agent: string,
): ConversationState {
  // The SDK raises a typed read failure after this callback, enabling safe
  // reconnection without acknowledging a cursor or discarding applied history.
  if (frame.type === 'cma.agent.error') return state;
  if (frame.data.agent_id !== agent || frame.data.incarnation !== state.agentIncarnation)
    throw new Error('Agent replay changed conversation identity.');
  if (frame.type === 'cma.agent.event') return foldV2Item(state, frame.data);
  if (frame.type === 'cma.agent.live_gap') {
    if (state.agentStatus === 'stopped' || state.agentStatus === 'deleted') return state;
    const runId =
      frame.data.turn_sequence === null ? state.latestTurnId : String(frame.data.turn_sequence);
    if (runId !== undefined && state.turnOutcomes?.[runId] !== undefined) return state;
    // This is also the initial stream handshake after a complete snapshot.
    // An active server turn alone is not evidence that this view lost anything.
    if (
      frame.data.reason === 'reconnected' &&
      !state.messages.some((message) => message.liveOnly) &&
      state.reasoning.length === 0
    )
      return state;
    return gap(
      state,
      `live-gap:${state.cursor}`,
      state.activeRunId === undefined
        ? 'The live connection was interrupted. Recorded messages and tool activity are retained.'
        : 'Recorded messages and tool activity are retained. The latest reply is being restored.',
      true,
    );
  }
  const event = frame.data;
  if (String(event.turn_sequence) !== state.activeRunId || event.after_sequence > state.cursor)
    throw new Error('A live update did not match the observed Agent turn.');
  if (state.liveGapRuns?.has(String(event.turn_sequence))) return state;
  if (
    event.kind === 'text_delta' &&
    state.messages.some(
      (message) =>
        message.role === 'assistant' &&
        !message.liveOnly &&
        (message.sequence ?? 0) > event.after_sequence,
    )
  )
    return state;
  return foldLiveEvent(
    clearLiveRecovery(state, state.activeRunId, false),
    {
      type: event.kind === 'text_delta' ? 'TEXT_MESSAGE_CONTENT' : 'REASONING_MESSAGE_CONTENT',
      threadId: agent,
      runId: String(event.turn_sequence),
      messageId: event.message_id,
      delta: event.delta,
      timestamp: event.observed_at,
      'cma:live': true,
      'cma:after': event.after_sequence,
    },
    agent,
  );
}
