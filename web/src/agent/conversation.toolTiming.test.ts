import { expect, it, vi } from 'vitest';
import type { AgentEvent, AgentLiveEvent } from '../api/types';
import {
  conversationFromTranscript,
  foldAgentEvent,
  foldLiveEvent as foldLive,
} from './conversation';
import { mergeTranscriptSnapshot } from './chronology';
import { summarizeTools } from './toolSummary';

const foldLiveEvent = (
  state: ReturnType<typeof conversationFromTranscript>,
  event: AgentLiveEvent,
) => foldLive(state, event, 'fixture-agent');
const empty = () =>
  conversationFromTranscript({ agentId: 'fixture-agent', throughSeq: 0, turns: [] });
const start = '2026-11-05T14:33:00.000Z';
const end = '2026-11-05T14:33:00.550Z';
const events = (finish: string): AgentEvent[] => [
  { seq: 1, appendedAt: start, kind: { type: 'turn_started', turn_sequence: 1 } },
  {
    seq: 2,
    appendedAt: start,
    kind: {
      type: 'tool_call_started',
      call_id: 'tool',
      title: 'Check fixture',
      kind: 'execute',
      status: 'running',
    },
  },
  {
    seq: 3,
    appendedAt: finish,
    kind: { type: 'tool_call_updated', call_id: 'tool', status: 'completed' },
  },
  { seq: 4, appendedAt: finish, kind: { type: 'turn_completed', turn_sequence: 1 } },
];
const live = (type: string, timestamp?: string): AgentLiveEvent => ({
  type,
  threadId: 'fixture-agent',
  runId: '1',
  toolCallId: 'tool',
  toolCallName: 'Check fixture',
  'cma:toolKind': 'execute',
  timestamp,
});

it.each([
  [start, 'Duration unavailable'],
  [end, '0.6s'],
])('keeps %s timing through pagination, duplicates, live takeover and reload', (finish, label) => {
  const history = events(finish);
  const firstPage = history.slice(0, 2).reduce(foldAgentEvent, empty());
  expect(summarizeTools(firstPage.toolCalls).duration).toBeUndefined();
  const restored = history.slice(2).reduce(foldAgentEvent, firstPage);
  expect(restored.toolCalls).toHaveLength(1);
  expect(summarizeTools(restored.toolCalls).duration).toBe(label);
  expect(history.reduce(foldAgentEvent, restored)).toEqual(restored);
  let streamed = foldLiveEvent(empty(), live('TOOL_CALL_START', start));
  streamed = foldLiveEvent(streamed, live('TOOL_CALL_END', finish));
  expect(streamed.toolCalls).toHaveLength(1);
  expect(summarizeTools(streamed.toolCalls).duration).toBe(label);
  const known = streamed.toolCalls[0];
  streamed = foldLiveEvent(streamed, live('TOOL_CALL_START', '2026-11-05T14:35:00Z'));
  streamed = foldLiveEvent(streamed, live('TOOL_CALL_END', '2026-11-05T14:36:00Z'));
  expect(streamed.toolCalls[0]).toEqual(known);
  const takeover = history.reduce(foldAgentEvent, streamed);
  expect(takeover.toolCalls).toHaveLength(1);
  expect(takeover.toolCalls[0]).toMatchObject({
    status: 'completed',
    startedAt: start,
    finishedAt: finish,
  });
  expect(summarizeTools(takeover.toolCalls).duration).toBe(label);
  expect(summarizeTools(mergeTranscriptSnapshot(takeover, restored).toolCalls).duration).toBe(
    label,
  );
  expect(summarizeTools(history.reduce(foldAgentEvent, empty()).toolCalls).duration).toBe(label);
});

it('does not replace absent event timestamps with browser receipt time or command text', () => {
  vi.useFakeTimers();
  try {
    vi.setSystemTime('2030-01-01T00:00:00Z');
    let state = foldLiveEvent(empty(), { ...live('TOOL_CALL_START'), toolCallName: 'sleep 1' });
    vi.advanceTimersByTime(1008);
    state = foldLiveEvent(state, live('TOOL_CALL_END'));
    expect(state.toolCalls[0]).toMatchObject({
      status: 'completed',
      startedAt: undefined,
      finishedAt: undefined,
    });
    expect(summarizeTools(state.toolCalls).duration).toBe('Duration unavailable');
  } finally {
    vi.useRealTimers();
  }
});
