import type {
  AgentEvent,
  AgentLiveEvent,
  AgentStatus,
  AgentTranscript,
  AgentUsage,
} from '../api/types';
import { orderEntries } from './chronology';
import { durableUsage } from './usage';
import { consumeLiveMessage, liveMessage } from './liveMessage';
import { liveTool, updateTool } from './toolCalls';
import { descriptionSource, mergeRuntimeMetadata } from './runtimeMetadata';
import { sessionCommands } from './slashCommands';
import { number, object, text } from './conversationValues';
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

function messageStatus(outcome: Readonly<Record<string, unknown>>) {
  return outcome.type === 'failed' ? 'error' : 'complete';
}

export function conversationFromTranscript(transcript: AgentTranscript): ConversationState {
  const messages = new Map<string, ConversationMessage>();
  for (const turn of transcript.turns) {
    for (const message of turn.messages) {
      const id = `event:${String(message.sequence)}`;
      // A queued pre-write retry keeps the admitted event but belongs to the latest turn.
      messages.set(id, {
        id,
        runId: String(turn.turnSequence),
        role: message.role,
        content: message.content,
        status: message.role === 'user' ? 'complete' : messageStatus(turn.outcome),
        at: message.appendedAt,
        order: message.sequence,
        sequence: message.sequence,
      });
    }
  }
  const activeTurn = transcript.turns.find((turn) => turn.outcome.type === 'active');
  const recovery: RecoveryNotice[] = transcript.turns.flatMap((turn) => {
    const failure = object(turn.outcome.failure);
    if (turn.outcome.type !== 'failed') return [];
    return [
      {
        id: `turn-error:${String(turn.turnSequence)}`,
        runId: String(turn.turnSequence),
        afterSequence: Math.max(
          turn.turnSequence,
          ...turn.messages.map((message) => message.sequence),
        ),
        title: 'Agent run failed',
        detail: text(failure?.detail) ?? 'The Agent run did not complete.',
        code: text(failure?.class),
      },
    ];
  });
  return {
    messages: [...messages.values()].sort((left, right) => left.order! - right.order!),
    toolCalls: [],
    reasoning: [],
    approvals: [],
    recovery,
    activeRunId: activeTurn === undefined ? undefined : String(activeTurn.turnSequence),
    activeRunStartedAt: activeTurn?.startedAt,
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

export function foldAgentEvent(state: ConversationState, event: AgentEvent): ConversationState {
  return orderEntries(state, foldDurableEvent(state, event), event.seq);
}

function foldDurableEvent(state: ConversationState, event: AgentEvent): ConversationState {
  if (event.seq <= state.cursor) return state;
  const kind = event.kind;
  const next = { ...state, cursor: event.seq };
  switch (kind.type) {
    case 'available_commands_set':
      return { ...next, availableCommands: sessionCommands(kind.commands) };
    case 'runtime_generation_retired':
      return { ...next, availableCommands: undefined };
    case 'conversation_opened':
    case 'turn_started':
      return {
        ...next,
        recovery: next.recovery.filter(
          (notice) => !notice.id.startsWith('sign-in:') && notice.id !== 'live-sign-in',
        ),
        activeRunId:
          kind.type === 'turn_started'
            ? String(number(kind.turn_sequence) ?? event.seq)
            : next.activeRunId,
        activeRunStartedAt:
          kind.type === 'turn_started' ? event.appendedAt : next.activeRunStartedAt,
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
      return updateTool(
        next,
        {
          id,
          runId: next.activeRunId,
          title: text(kind.title) ?? 'Tool call',
          kind: text(kind.kind),
          status: text(kind.status) ?? 'running',
        },
        true,
      );
    }
    case 'tool_call_updated': {
      const id = text(kind.call_id);
      const current = next.toolCalls.find((tool) => tool.id === id);
      if (id === undefined || current === undefined) return next;
      return updateTool(next, {
        ...current,
        title: text(kind.title) ?? current.title,
        status: text(kind.status) ?? current.status,
      });
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
      return durableUsage(next, {
        ...kind,
        input_tokens: Math.max(next.usage?.inputTokens ?? 0, number(kind.input_tokens) ?? 0),
        output_tokens: Math.max(next.usage?.outputTokens ?? 0, number(kind.output_tokens) ?? 0),
      });
    case 'usage_updated':
      return durableUsage(next, kind);
    case 'runtime_metadata_changed': {
      const metadata = object(kind.metadata);
      return metadata === undefined
        ? next
        : {
            ...next,
            runtimeMetadata: mergeRuntimeMetadata(next.runtimeMetadata, {
              workingDirectory: text(metadata.working_directory),
              description: text(metadata.description),
              descriptionSource: descriptionSource(metadata.description_source),
              descriptionIsSetupPreview:
                typeof metadata.description_is_setup_preview === 'boolean'
                  ? metadata.description_is_setup_preview
                  : undefined,
            }),
          };
    }
    case 'conversation_gap':
      return recover(next, {
        id: `gap:${String(event.seq)}`,
        title: 'Incomplete conversation output',
        detail: `${String(number(kind.frames) ?? 0)} live frame(s) were lost. Stored messages remain available, but the output may be incomplete.`,
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
      return {
        ...next,
        agentStatus: text(kind.to) as AgentStatus | undefined,
        agentStatusAt: event.appendedAt,
      };
    case 'turn_completed':
    case 'turn_interrupted':
      return { ...next, activeRunId: undefined, activeRunStartedAt: undefined, reasoning: [] };
    case 'turn_failed': {
      const failure = object(kind.failure);
      return recover(
        { ...next, activeRunId: undefined, activeRunStartedAt: undefined, reasoning: [] },
        {
          id: `run-error:${String(event.seq)}`,
          runId:
            number(kind.turn_sequence) === undefined
              ? next.activeRunId
              : String(kind.turn_sequence),
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

function patchState(
  state: ConversationState,
  patch: unknown,
  timestamp?: string,
): ConversationState {
  const item = object(patch);
  if (item === undefined) return state;
  const path = text(item.path);
  const operation = text(item.op);
  if (path === undefined || operation === undefined) return state;
  if (path === '/availableCommands')
    return {
      ...state,
      availableCommands: operation === 'remove' ? undefined : sessionCommands(item.value),
    };
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
  if (path === '/runtimeMetadata') {
    const metadata = object(item.value);
    return metadata === undefined
      ? state
      : {
          ...state,
          runtimeMetadata: mergeRuntimeMetadata(state.runtimeMetadata, {
            workingDirectory: text(metadata.workingDirectory),
            description: text(metadata.description),
            descriptionSource: descriptionSource(metadata.descriptionSource),
            descriptionIsSetupPreview:
              typeof metadata.descriptionIsSetupPreview === 'boolean'
                ? metadata.descriptionIsSetupPreview
                : undefined,
          }),
        };
  }
  if (path === '/usage') {
    const usage = object(item.value);
    const turn = object(usage?.currentTurn);
    return usage === undefined
      ? state
      : durableUsage(state, {
          input_tokens: usage.inputTokens,
          output_tokens: usage.outputTokens,
          context_used: usage.contextUsed,
          context_window: usage.contextWindow,
          turn_sequence: turn?.turnSequence,
          turn_input_tokens: turn?.inputTokens,
          turn_output_tokens: turn?.outputTokens,
        });
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
    if (
      timestamp === undefined ||
      !Number.isFinite(Date.parse(timestamp)) ||
      (state.agentStatusAt !== undefined && Date.parse(timestamp) < Date.parse(state.agentStatusAt))
    )
      return state;
    return { ...state, agentStatus: text(item.value) as AgentStatus, agentStatusAt: timestamp };
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
  return delta.reduce<ConversationState>(
    (current, patch) => patchState(current, patch, event.timestamp),
    state,
  );
}

export function foldLiveEvent(
  state: ConversationState,
  event: AgentLiveEvent,
  threadId: string,
): ConversationState {
  return orderEntries(state, foldLive(state, event, threadId), number(event.cursor));
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
  if (event.type === 'RUN_STARTED')
    return {
      ...next,
      activeRunId: event.runId,
      activeRunStartedAt:
        next.activeRunId === event.runId
          ? (next.activeRunStartedAt ?? event.timestamp)
          : event.timestamp,
    };
  if (event.type === 'RUN_FINISHED') {
    const outcome = object(event.outcome);
    const interrupts = Array.isArray(outcome?.interrupts) ? outcome.interrupts : [];
    const approvals = interrupts
      .map(approvalFrom)
      .filter((approval): approval is ApprovalRequest => approval !== undefined)
      .reduce((items, approval) => upsert(items, approval), next.approvals);
    return {
      ...next,
      approvals,
      reasoning: [],
      activeRunId: undefined,
      activeRunStartedAt: undefined,
    };
  }
  if (event.type === 'RUN_ERROR') {
    if (['owner', 'operator', 'sandbox_retired', 'agent_deleted'].includes(text(event.code) ?? ''))
      return { ...next, activeRunId: undefined, activeRunStartedAt: undefined, reasoning: [] };
    return recover(
      { ...next, activeRunId: undefined, activeRunStartedAt: undefined, reasoning: [] },
      {
        id: `live-error:${event.runId}`,
        runId: event.runId,
        title: 'Agent run failed',
        detail: text(event.message) ?? 'The Agent run did not complete.',
        code: text(event.code),
      },
    );
  }
  return next;
}
