import type {
  AgentEvent,
  AgentLiveEvent,
  AgentStatus,
  AgentTranscript,
  AgentUsage,
} from '../api/types';
import { orderEntries } from './chronology';
import { consumeLiveMessage, liveMessage } from './liveMessage';
import type {
  ApprovalRequest,
  ConversationMessage,
  ConversationState,
  RecoveryNotice,
} from './model';
export type {
  ApprovalRequest,
  ConversationMessage,
  ConversationState,
  RecoveryNotice,
} from './model';

function object(value: unknown): Readonly<Record<string, unknown>> | undefined {
  return typeof value === 'object' && value !== null && !Array.isArray(value)
    ? (value as Readonly<Record<string, unknown>>)
    : undefined;
}

function text(value: unknown): string | undefined {
  return typeof value === 'string' ? value : undefined;
}

function number(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function messageStatus(outcome: Readonly<Record<string, unknown>>) {
  return outcome.type === 'failed' ? 'error' : 'complete';
}

export function conversationFromTranscript(transcript: AgentTranscript): ConversationState {
  const messages = transcript.turns.flatMap((turn) => {
    const runId = String(turn.turnSequence);
    const owner: ConversationMessage = {
      id: `turn:${runId}:owner`,
      runId,
      role: 'user',
      content: turn.ownerMessage,
      status: 'complete',
      at: turn.startedAt,
    };
    const assistants: ConversationMessage[] = turn.agentMessages.map((content, index) => ({
      id: `turn:${runId}:agent:${String(index)}`,
      runId,
      role: 'assistant',
      content,
      status: messageStatus(turn.outcome),
      at: turn.startedAt,
    }));
    return [owner, ...assistants];
  });
  const recovery: RecoveryNotice[] = transcript.turns.flatMap((turn) => {
    const failure = object(turn.outcome.failure);
    if (turn.outcome.type !== 'failed') return [];
    return [
      {
        id: `turn-error:${String(turn.turnSequence)}`,
        title: 'Agent run failed',
        detail: text(failure?.detail) ?? 'The Agent run did not complete.',
        code: text(failure?.class),
      },
    ];
  });
  return {
    messages,
    toolCalls: [],
    reasoning: [],
    approvals: [],
    recovery,
    cursor: transcript.throughSeq,
  };
}

function upsert<T extends { readonly id: string }>(items: readonly T[], item: T): readonly T[] {
  const at = items.findIndex(({ id }) => id === item.id);
  if (at < 0) return [...items, item];
  return items.map((current, index) => (index === at ? item : current));
}

function recover(state: ConversationState, notice: RecoveryNotice): ConversationState {
  return { ...state, recovery: upsert(state.recovery, notice) };
}

function durableUsage(state: ConversationState, kind: Readonly<Record<string, unknown>>) {
  const inputTokens = number(kind.input_tokens);
  const outputTokens = number(kind.output_tokens);
  const contextUsed = number(kind.context_used);
  if (inputTokens === undefined || outputTokens === undefined || contextUsed === undefined) {
    return state;
  }
  return {
    ...state,
    usage: {
      agentId: state.usage?.agentId ?? '',
      throughSeq: state.cursor,
      inputTokens,
      outputTokens,
      contextUsed,
      contextWindow: number(kind.context_window),
    },
  };
}

export function foldAgentEvent(state: ConversationState, event: AgentEvent): ConversationState {
  return orderEntries(state, foldDurableEvent(state, event));
}

function foldDurableEvent(state: ConversationState, event: AgentEvent): ConversationState {
  if (event.seq <= state.cursor) return state;
  const kind = event.kind;
  const next = { ...state, cursor: event.seq };
  switch (kind.type) {
    case 'turn_started':
      return {
        ...next,
        recovery: next.recovery.filter(
          (notice) => !notice.id.startsWith('sign-in:') && notice.id !== 'live-sign-in',
        ),
        activeRunId: String(number(kind.turn_sequence) ?? event.seq),
      };
    case 'owner_message':
    case 'agent_message': {
      const content = text(kind.content);
      if (content === undefined) return next;
      const reconciled =
        kind.type === 'agent_message'
          ? consumeLiveMessage(next.messages, next.activeRunId)
          : { messages: next.messages, order: undefined };
      return {
        ...next,
        messages: upsert(reconciled.messages, {
          id: `event:${String(event.seq)}`,
          runId: next.activeRunId,
          role: kind.type === 'owner_message' ? 'user' : 'assistant',
          content,
          status: 'complete',
          at: event.appendedAt,
          order: reconciled.order,
        }),
      };
    }
    case 'tool_call_started': {
      const id = text(kind.call_id);
      if (id === undefined) return next;
      return {
        ...next,
        toolCalls: upsert(next.toolCalls, {
          id,
          runId: next.activeRunId,
          title: text(kind.title) ?? 'Tool call',
          kind: text(kind.kind),
          status: text(kind.status) ?? 'running',
        }),
      };
    }
    case 'tool_call_updated': {
      const id = text(kind.call_id);
      const current = next.toolCalls.find((tool) => tool.id === id);
      if (id === undefined || current === undefined) return next;
      return {
        ...next,
        toolCalls: upsert(next.toolCalls, {
          ...current,
          title: text(kind.title) ?? current.title,
          status: text(kind.status) ?? current.status,
        }),
      };
    }
    case 'approval_requested': {
      const id = text(kind.approval_id);
      if (id === undefined) return next;
      return {
        ...next,
        approvals: upsert(next.approvals, {
          id,
          reason: text(kind.action) ?? 'confirmation',
          message: text(kind.detail),
        }),
      };
    }
    case 'approval_resolved': {
      const id = text(kind.approval_id);
      const current = next.approvals.find((approval) => approval.id === id);
      if (id === undefined || current === undefined) return next;
      return {
        ...next,
        approvals: upsert(next.approvals, {
          ...current,
          decision: text(kind.decision) ?? 'resolved',
        }),
      };
    }
    case 'usage':
      return durableUsage(next, kind);
    case 'conversation_gap':
      return recover(next, {
        id: `gap:${String(event.seq)}`,
        title: 'Conversation recovery',
        detail: `${String(number(kind.frames) ?? 0)} live frame(s) were recovered from durable history.`,
      });
    case 'sign_in_required':
      return recover(next, {
        id: `sign-in:${String(event.seq)}`,
        title: 'Sign in required',
        detail: text(kind.message) ?? 'The Agent needs an authenticated provider session.',
        url: text(kind.url),
        code: text(kind.code),
      });
    case 'first_message_changed':
      return kind.state === 'recovery_required'
        ? recover(next, {
            id: 'first-message-recovery',
            title: 'First message needs recovery',
            detail: 'Delivery was interrupted before execution could be confirmed.',
          })
        : next;
    case 'status_changed':
      return { ...next, agentStatus: text(kind.to) as AgentStatus | undefined };
    case 'turn_completed':
    case 'turn_interrupted':
      return { ...next, activeRunId: undefined };
    case 'turn_failed': {
      const failure = object(kind.failure);
      return recover(
        { ...next, activeRunId: undefined },
        {
          id: `run-error:${String(event.seq)}`,
          title: 'Agent run failed',
          detail: text(failure?.detail) ?? 'The Agent run did not complete.',
        },
      );
    }
    default:
      return next;
  }
}

function liveReasoning(state: ConversationState, event: AgentLiveEvent): ConversationState {
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

function liveTool(state: ConversationState, event: AgentLiveEvent): ConversationState {
  const id = text(event.toolCallId);
  if (id === undefined) return state;
  const current = state.toolCalls.find((tool) => tool.id === id);
  if (event.type === 'TOOL_CALL_START') {
    return {
      ...state,
      toolCalls: upsert(state.toolCalls, {
        id,
        runId: event.runId,
        title: text(event.toolCallName) ?? 'Tool call',
        kind: text(event['cma:toolKind']),
        status: text(event['cma:status']) ?? 'running',
      }),
    };
  }
  if (current === undefined) return state;
  const update = text(event.delta);
  const detail =
    event.type === 'TOOL_CALL_ARGS' && update !== undefined
      ? `${current.detail ?? ''}${update}`
      : (text(event.result) ?? update ?? current.detail);
  return {
    ...state,
    toolCalls: upsert(state.toolCalls, {
      ...current,
      status:
        text(event['cma:status']) ??
        (event.type === 'TOOL_CALL_END' ? 'completed' : current.status),
      detail,
    }),
  };
}

function approvalFrom(value: unknown): ApprovalRequest | undefined {
  const item = object(value);
  const id = text(item?.id);
  if (item === undefined || id === undefined) return undefined;
  return {
    id,
    reason: text(item.reason) ?? 'confirmation',
    message: text(item.message),
  };
}

function patchState(state: ConversationState, patch: unknown): ConversationState {
  const item = object(patch);
  if (item === undefined) return state;
  const path = text(item.path);
  const operation = text(item.op);
  if (path === undefined || operation === undefined) return state;
  if (path.startsWith('/pendingApprovals/')) {
    const id = path.slice('/pendingApprovals/'.length).replaceAll('~1', '/').replaceAll('~0', '~');
    if (operation === 'remove') {
      const current = state.approvals.find((approval) => approval.id === id);
      return current === undefined
        ? state
        : { ...state, approvals: upsert(state.approvals, { ...current, decision: 'resolved' }) };
    }
    const approval = approvalFrom(item.value);
    return approval === undefined
      ? state
      : { ...state, approvals: upsert(state.approvals, approval) };
  }
  if (path.startsWith('/usage/')) {
    const field = path.slice('/usage/'.length);
    const usage = state.usage ?? {
      agentId: '',
      throughSeq: state.cursor,
      inputTokens: 0,
      outputTokens: 0,
      contextUsed: 0,
    };
    const value = number(item.value);
    if (value === undefined) return state;
    const fields: Readonly<Record<string, keyof AgentUsage>> = {
      inputTokens: 'inputTokens',
      outputTokens: 'outputTokens',
      contextUsed: 'contextUsed',
      contextWindow: 'contextWindow',
    };
    const key = fields[field];
    return key === undefined ? state : { ...state, usage: { ...usage, [key]: value } };
  }
  if (path === '/status' && text(item.value) !== undefined) {
    return { ...state, agentStatus: text(item.value) as AgentStatus };
  }
  if (path === '/cma/sandbox') {
    const sandbox = object(item.value);
    return { ...state, livePausedNotice: text(sandbox?.pausedNotice) ?? null };
  }
  if (path === '/gaps/-') {
    const gap = object(item.value);
    return recover(state, {
      id: `live-gap:${String(state.recovery.length)}`,
      title: 'Conversation recovery',
      detail: `${String(number(gap?.frames) ?? 0)} live frame(s) were recovered from durable history.`,
    });
  }
  if (path === '/signIn') {
    if (item.value === null)
      return {
        ...state,
        recovery: state.recovery.filter(
          (notice) => !notice.id.startsWith('sign-in:') && notice.id !== 'live-sign-in',
        ),
      };
    const signIn = object(item.value);
    return recover(state, {
      id: 'live-sign-in',
      title: 'Sign in required',
      detail: text(signIn?.message) ?? 'The Agent needs an authenticated provider session.',
      url: text(signIn?.url),
      code: text(signIn?.code),
    });
  }
  return state;
}

function liveState(state: ConversationState, event: AgentLiveEvent): ConversationState {
  const delta = Array.isArray(event.delta) ? event.delta : [];
  return delta.reduce(patchState, state);
}

export function foldLiveEvent(
  state: ConversationState,
  event: AgentLiveEvent,
  threadId: string,
): ConversationState {
  return orderEntries(state, foldLive(state, event, threadId));
}

function foldLive(
  state: ConversationState,
  event: AgentLiveEvent,
  threadId: string,
): ConversationState {
  if (event.threadId !== threadId) return state;
  const cursor = number(event.cursor);
  const next = cursor === undefined || cursor <= state.cursor ? state : { ...state, cursor };
  if (event.type.startsWith('TEXT_MESSAGE_')) return liveMessage(next, event);
  if (event.type.startsWith('TOOL_CALL_')) return liveTool(next, event);
  if (event.type.includes('REASONING') || event.type.startsWith('THINKING_')) {
    return liveReasoning(next, event);
  }
  if (event.type === 'STATE_DELTA') return liveState(next, event);
  if (event.type === 'RUN_STARTED') return { ...next, activeRunId: event.runId };
  if (event.type === 'RUN_FINISHED') {
    const outcome = object(event.outcome);
    const interrupts = Array.isArray(outcome?.interrupts) ? outcome.interrupts : [];
    const approvals = interrupts
      .map(approvalFrom)
      .filter((approval): approval is ApprovalRequest => approval !== undefined)
      .reduce((items, approval) => upsert(items, approval), next.approvals);
    const reasoning = next.reasoning.map((block) =>
      block.runId === event.runId ? { ...block, status: 'complete' as const } : block,
    );
    return { ...next, approvals, reasoning, activeRunId: undefined };
  }
  if (event.type === 'RUN_ERROR') {
    return recover(
      { ...next, activeRunId: undefined },
      {
        id: `live-error:${event.runId}`,
        title: 'Agent run failed',
        detail: text(event.message) ?? 'The Agent run did not complete.',
        code: text(event.code),
      },
    );
  }
  return next;
}
