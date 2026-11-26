import type { AgentEvent, AgentLiveEvent, AgentStatus, AgentUsage } from '../api/types';
import { orderEntries } from './chronology';
export { conversationFromTranscript } from './transcript';
import { durableUsage } from './usage';
import { clearLiveGapRun, durableAssistantMessage, liveMessage } from './liveMessage';
import { liveTool, updateTool } from './toolCalls';
import { setPlan } from './plans';
import { descriptionSource, durableRuntimeMetadata, mergeRuntimeMetadata } from './runtimeMetadata';
import { sessionCommands } from './slashCommands';
import { number, object, text, upsert } from './conversationValues';
import { liveReasoning } from './liveReasoning';
import { startTurn, finishTurn, endTurn } from './turnTiming';
import { failedTurn, hasRecordedGap, recover } from './recovery';
import { failureMessage } from './failureMessage';
import type { ApprovalRequest, ConversationState } from './model';
export type {
  ApprovalRequest,
  ConversationMessage,
  ConversationState,
  RecoveryNotice,
} from './model';

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
        ...(kind.type === 'turn_started'
          ? startTurn(next, String(number(kind.turn_sequence) ?? event.seq), event.appendedAt)
          : next),
        recovery: next.recovery.filter(
          (notice) => !notice.id.startsWith('sign-in:') && notice.id !== 'live-sign-in',
        ),
      };
    case 'owner_message':
    case 'agent_message': {
      const content = text(kind.content);
      if (content === undefined) return next;
      const messages =
        kind.type === 'agent_message'
          ? durableAssistantMessage(next.messages, next.activeRunId, {
              id: `event:${String(event.seq)}`,
              runId: next.activeRunId,
              role: 'assistant',
              content,
              status: 'complete',
              at: event.appendedAt,
            })
          : upsert(next.messages, {
              id: `event:${String(event.seq)}`,
              runId: next.activeRunId,
              role: 'user',
              content,
              status: 'complete',
              at: event.appendedAt,
            });
      return {
        ...next,
        messages,
        liveGapRuns:
          kind.type === 'agent_message' && next.activeRunId !== undefined
            ? clearLiveGapRun(next.liveGapRuns, next.activeRunId)
            : next.liveGapRuns,
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
          sequence: event.seq,
          kind: text(kind.kind),
          status: text(kind.status) ?? 'running',
          startedAt: event.appendedAt,
        },
        true,
      );
    }
    case 'tool_output_reported':
    case 'tool_call_updated': {
      const id = text(kind.call_id);
      const current = next.toolCalls.find((tool) => tool.id === id);
      if (id === undefined || current === undefined) return next;
      const output = object(kind.output);
      return updateTool(
        next,
        {
          ...current,
          title: text(kind.title) ?? current.title,
          status: text(kind.status) ?? current.status,
          finishedAt: ['completed', 'failed', 'cancelled', 'error'].includes(
            text(kind.status) ?? '',
          )
            ? (current.finishedAt ?? event.appendedAt)
            : current.finishedAt,
          output:
            typeof output?.text === 'string' && typeof output.truncated === 'boolean'
              ? { text: output.text, truncated: output.truncated }
              : current.output,
        },
        false,
        event.appendedAt,
      );
    }
    case 'approval_requested': {
      const id = text(kind.approval_id);
      if (id === undefined) return next;
      if (next.approvals.some((approval) => approval.id === id && approval.source === 'external'))
        throw new Error('Native approval conflicts with an external approval identity.');
      return {
        ...next,
        approvals: upsert(next.approvals, {
          id,
          runId: next.activeRunId,
          reason: text(kind.action) ?? 'confirmation',
          message: text(kind.detail),
        }),
      };
    }
    case 'approval_resolved': {
      const id = text(kind.approval_id);
      const current = next.approvals.find(
        (approval) => approval.id === id && approval.source !== 'external',
      );
      if (id === undefined || current === undefined) return next;
      return {
        ...next,
        approvals: upsert(next.approvals, {
          ...current,
          decision: text(kind.decision) ?? 'resolved',
        }),
      };
    }
    case 'plan_set':
      return setPlan(next, next.activeRunId, kind.entries, event.appendedAt);
    case 'usage':
      return durableUsage(next, {
        ...kind,
        input_tokens: Math.max(next.usage?.inputTokens ?? 0, number(kind.input_tokens) ?? 0),
        output_tokens: Math.max(next.usage?.outputTokens ?? 0, number(kind.output_tokens) ?? 0),
      });
    case 'usage_updated':
      return durableUsage(next, kind);
    case 'title_set':
      if (text(kind.title) === undefined) return next;
      return kind.source === 'owner_override'
        ? { ...next, ownerTitle: text(kind.title) }
        : { ...next, title: text(kind.title) };
    case 'runtime_metadata_changed':
      return {
        ...next,
        runtimeMetadata: mergeRuntimeMetadata(
          next.runtimeMetadata,
          durableRuntimeMetadata(kind.metadata),
        ),
      };
    case 'conversation_gap':
      return recover(next, {
        id: `gap:${String(event.seq)}`,
        runId: next.activeRunId,
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
    case 'turn_completed': {
      const runId = number(kind.turn_sequence)?.toString() ?? next.activeRunId;
      const incomplete = hasRecordedGap(next, runId);
      const finalMessage = next.messages.reduce<ConversationState['messages'][number] | undefined>(
        (latest, message) =>
          message.runId === runId &&
          !message.liveOnly &&
          message.sequence !== undefined &&
          (latest === undefined || message.sequence > latest.sequence!)
            ? message
            : latest,
        undefined,
      );
      return {
        ...endTurn(next, runId, event.appendedAt),
        liveGapRuns:
          runId === undefined ? next.liveGapRuns : clearLiveGapRun(next.liveGapRuns, runId),
        turnOutcomes: {
          ...next.turnOutcomes,
          [runId ?? 'unattributed']: incomplete ? 'failed' : 'completed',
        },
        messages: next.messages.map((message) =>
          message === finalMessage &&
          message.role === 'assistant' &&
          message.content.trim() !== '' &&
          !incomplete
            ? { ...message, finalAnswer: true }
            : message,
        ),
      };
    }
    case 'turn_interrupted':
      return {
        ...endTurn(
          next,
          number(kind.turn_sequence)?.toString() ?? next.activeRunId,
          event.appendedAt,
        ),
        turnOutcomes: {
          ...next.turnOutcomes,
          [number(kind.turn_sequence)?.toString() ?? next.activeRunId ?? 'unattributed']:
            'interrupted',
        },
        liveGapRuns: clearLiveGapRun(
          next.liveGapRuns,
          number(kind.turn_sequence)?.toString() ?? next.activeRunId ?? '',
        ),
      };
    case 'turn_failed':
      return failedTurn(next, event);
    default:
      return next;
  }
}

function approvalFrom(value: unknown, runId?: string): ApprovalRequest | undefined {
  const item = object(value);
  const id = text(item?.id);
  if (item === undefined || id === undefined) return undefined;
  return {
    id,
    runId,
    reason: text(item.reason) ?? 'confirmation',
    message: text(item.message),
  };
}

function patchState(
  state: ConversationState,
  patch: unknown,
  timestamp?: string,
  runId?: string,
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
  if (path === '/plan')
    return setPlan(state, runId, operation === 'remove' ? [] : item.value, timestamp);
  if (path.startsWith('/pendingApprovals/')) {
    const id = path.slice('/pendingApprovals/'.length).replaceAll('~1', '/').replaceAll('~0', '~');
    if (operation === 'remove') {
      const current = state.approvals.find(
        (approval) => approval.id === id && approval.source !== 'external',
      );
      return current === undefined
        ? state
        : { ...state, approvals: upsert(state.approvals, { ...current, decision: 'resolved' }) };
    }
    const approval = approvalFrom(item.value, runId);
    if (
      approval &&
      state.approvals.some((current) => current.id === approval.id && current.source === 'external')
    )
      throw new Error('Native approval conflicts with an external approval identity.');
    return approval === undefined
      ? state
      : { ...state, approvals: upsert(state.approvals, approval) };
  }
  if (path === '/ownerTitle') return { ...state, ownerTitle: text(item.value) };
  if (path === '/title') return { ...state, title: text(item.value) };
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
    (current, patch) => patchState(current, patch, event.timestamp, event.runId),
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
  if (event.type === 'STATE_DELTA' && cursor !== undefined && cursor <= state.cursor) return state;
  const next = cursor === undefined || cursor <= state.cursor ? state : { ...state, cursor };
  if (event.type.startsWith('TEXT_MESSAGE_')) return liveMessage(next, event);
  if (event.type === 'CUSTOM' && event.name === 'cma.tool_output_reported') {
    const value = object(event.value);
    return liveTool(next, { ...event, toolCallId: value?.toolCallId, 'cma:output': value?.output });
  }
  if (event.type.startsWith('TOOL_CALL_')) return liveTool(next, event);
  if (event.type.includes('REASONING') || event.type.startsWith('THINKING_')) {
    return liveReasoning(next, event);
  }
  if (event.type === 'STATE_DELTA') return liveState(next, event);
  if (event.type === 'RUN_STARTED') return startTurn(next, event.runId, event.timestamp);
  if (event.type === 'RUN_FINISHED') {
    const outcome = object(event.outcome);
    const interrupts = Array.isArray(outcome?.interrupts) ? outcome.interrupts : [];
    const approvals = interrupts
      .map((value) => approvalFrom(value, event.runId))
      .filter((approval): approval is ApprovalRequest => approval !== undefined)
      .reduce(
        (items, approval) =>
          items.some((item) => item.id === approval.id && item.source === 'external')
            ? items
            : upsert(items, approval),
        next.approvals,
      );
    return {
      ...(interrupts.length === 0 ? finishTurn(next, event.runId, event.timestamp) : next),
      turnOutcomes:
        interrupts.length === 0 && (outcome?.type === 'complete' || outcome?.type === 'interrupt')
          ? {
              ...next.turnOutcomes,
              [event.runId]:
                outcome.type === 'interrupt'
                  ? 'interrupted'
                  : hasRecordedGap(next, event.runId)
                    ? 'failed'
                    : 'completed',
            }
          : next.turnOutcomes,
      approvals,
      reasoning: [],
      activeRunId: interrupts.length === 0 ? undefined : event.runId,
      activeRunStartedAt: interrupts.length === 0 ? undefined : next.activeRunStartedAt,
    };
  }
  if (event.type === 'RUN_ERROR') {
    const cancelled = ['owner', 'operator', 'sandbox_retired', 'agent_deleted'].includes(
      text(event.code) ?? '',
    );
    const finished: ConversationState = {
      ...finishTurn(next, event.runId, event.timestamp),
      turnOutcomes: { ...next.turnOutcomes, [event.runId]: cancelled ? 'interrupted' : 'failed' },
    };
    if (cancelled)
      return { ...finished, activeRunId: undefined, activeRunStartedAt: undefined, reasoning: [] };
    return recover(
      { ...finished, activeRunId: undefined, activeRunStartedAt: undefined, reasoning: [] },
      {
        id: `live-error:${event.runId}`,
        runId: event.runId,
        title: 'Agent run failed',
        ...failureMessage(event.code),
        retainedOutput: event['cma:retainedOutput'],
      },
    );
  }
  return next;
}
