import { describe, expect, it } from 'vitest';
import type { AgentLiveEvent } from '../api/types';
import { AGENT_TRANSCRIPT } from '../test/agent';
import { conversationFromTranscript } from './conversation';
import { HydrationEvents } from './mergeHydration';
import type { ConversationState } from './model';

const snapshot = conversationFromTranscript(AGENT_TRANSCRIPT);
const empty = conversationFromTranscript({ agentId: 'agt_1', turns: [], throughSeq: 0 });
const frame = (type: string, fields: Record<string, unknown> = {}): AgentLiveEvent => ({
  type,
  threadId: 'agt_1',
  runId: '1',
  ...fields,
});
function apply(buffer: HydrationEvents, state: ConversationState, ...events: AgentLiveEvent[]) {
  return events.reduce((current, event) => buffer.apply(current, event, 'agt_1', true), state);
}
const message = (seq: number, content: string, runId = '1') => [
  frame('TEXT_MESSAGE_START', { messageId: `msg_${String(seq)}`, role: 'assistant', runId }),
  frame('TEXT_MESSAGE_CONTENT', { messageId: `msg_${String(seq)}`, delta: content, runId }),
  frame('TEXT_MESSAGE_END', { messageId: `msg_${String(seq)}`, cursor: seq, runId }),
];
const live = (delta: string, fields = {}) =>
  frame('TEXT_MESSAGE_CONTENT', {
    messageId: 'provider-text',
    'cma:live': true,
    delta,
    ...fields,
  });

describe('history and concurrent AG-UI hydration', () => {
  it('does not resurrect a completed turn or duplicate durable messages covered by history', () => {
    const buffer = new HydrationEvents();
    apply(
      buffer,
      empty,
      frame('RUN_STARTED', { cursor: 1 }),
      ...message(3, 'The repository is ready.'),
    );
    expect(buffer.merge(snapshot, 'agt_1', true)).toEqual(snapshot);
  });
  it('drops the rest of a partial durable message when history includes it before END arrives', () => {
    const buffer = new HydrationEvents();
    const [start, content, end] = message(3, 'The repository is ready.');
    apply(buffer, empty, frame('RUN_STARTED', { cursor: 1 }), start!, content!);
    const merged = buffer.merge(snapshot, 'agt_1', true);
    expect(apply(buffer, merged, content!, end!)).toEqual(snapshot);
  });
  it('excludes ephemeral text and reasoning from a completed historical turn', () => {
    const buffer = new HydrationEvents();
    apply(
      buffer,
      empty,
      frame('RUN_STARTED', { cursor: 1 }),
      live('Old answer'),
      frame('REASONING_MESSAGE_CONTENT', {
        'cma:live': true,
        messageId: 'thought',
        delta: 'Old thought',
      }),
    );
    expect(buffer.merge(snapshot, 'agt_1', true)).toEqual(snapshot);
  });
  it('retains a newer turn with coalesced live fragments exactly once', () => {
    const buffer = new HydrationEvents();
    apply(
      buffer,
      empty,
      frame('RUN_STARTED', { cursor: 5, runId: '5' }),
      live('New ', { runId: '5' }),
      live('answer', { runId: '5' }),
    );
    const merged = buffer.merge(snapshot, 'agt_1', true);
    expect(merged.activeRunId).toBe('5');
    expect(merged.messages.map((item) => item.content)).toEqual([
      'Inspect this repository.',
      'The repository is ready.',
      'New answer',
    ]);
    expect(buffer.merge(snapshot, 'agt_1', true)).toEqual(snapshot);
  });
  it('filters committed same-turn text by the actual stream cursor and keeps fresh text', () => {
    const active = { ...snapshot, activeRunId: '1' };
    const buffer = new HydrationEvents();
    apply(buffer, empty, frame('RUN_STARTED', { cursor: 1 }), live('The repository is ready.'));
    const merged = buffer.merge(active, 'agt_1', false);
    expect(merged.messages).toEqual(snapshot.messages);
    const next = apply(buffer, merged, message(3, '')[2]!, live('Next response'));
    expect(next.messages.at(-1)?.content).toBe('Next response');
    expect(buffer.merge(active, 'agt_1', true).messages.at(-1)?.content).toBe('Next response');
  });
  it('does not reopen an approval already resolved in the snapshot', () => {
    const buffer = new HydrationEvents();
    apply(
      buffer,
      empty,
      frame('RUN_STARTED', { cursor: 1 }),
      frame('RUN_FINISHED', {
        outcome: { type: 'interrupt', interrupts: [{ id: 'permission', reason: 'write' }] },
      }),
    );
    const resolved = {
      ...snapshot,
      approvals: [{ id: 'permission', reason: 'write', decision: 'allow' }],
    };
    expect(buffer.merge(resolved, 'agt_1', true)).toEqual(resolved);
  });
  it('accepts auxiliary stream metadata without an active run', () => {
    const buffer = new HydrationEvents();
    buffer.reset(snapshot);
    const next = apply(
      buffer,
      snapshot,
      frame('STATE_DELTA', {
        delta: [{ op: 'replace', path: '/runtimeMetadata', value: { description: 'New title' } }],
      }),
    );
    expect(next.runtimeMetadata?.description).toBe('New title');
    expect(next.activeRunId).toBeUndefined();
  });
  it('reset discards old buffered output and rejects events for another Agent', () => {
    const buffer = new HydrationEvents();
    apply(
      buffer,
      empty,
      frame('RUN_STARTED', { cursor: 5, runId: '5' }),
      live('Old account', { runId: '5' }),
    );
    buffer.reset();
    expect(
      buffer.apply(empty, frame('RUN_STARTED', { threadId: 'other', cursor: 8 }), 'agt_1', true),
    ).toBe(empty);
    expect(buffer.merge(snapshot, 'agt_1', true)).toEqual(snapshot);
  });
});
