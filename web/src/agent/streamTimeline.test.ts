import { expect, it, vi } from 'vitest';
import { conversationFromTranscript, foldLiveEvent } from './conversation';
import type { ConversationState } from './model';
import { createStreamTimeline } from './streamTimeline';

function history(): ConversationState {
  return {
    ...conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] }),
    messages: [
      {
        id: 'first',
        role: 'assistant',
        runId: 'old',
        content: 'History',
        status: 'complete',
        order: 1,
      },
      { id: 'reply', role: 'assistant', runId: 'live', content: '', status: 'streaming', order: 4 },
    ],
    toolCalls: [
      { id: 'one', title: 'One', status: 'completed', order: 2 },
      { id: 'two', title: 'Two', status: 'completed', order: 3 },
    ],
  };
}

it('does not sort or rebuild historical tool groups when only text changes', () => {
  const project = createStreamTimeline();
  const state = history();
  const baseline = project(state);
  const next = foldLiveEvent(
    state,
    {
      type: 'TEXT_MESSAGE_CONTENT',
      threadId: 'agent',
      runId: 'live',
      messageId: 'reply',
      delta: 'More',
    },
    'agent',
  );
  const sort = vi.spyOn(Array.prototype, 'sort');
  let projected: ReturnType<typeof project>;
  try {
    projected = project(next);
    expect(sort).not.toHaveBeenCalled();
  } finally {
    sort.mockRestore();
  }
  expect(projected.items[0]).toBe(baseline.items[0]);
  expect(projected.items[1]).toBe(baseline.items[1]);
  expect(projected.items[2]).toMatchObject({ kind: 'message', item: { content: 'More' } });
  expect(projected.continued).toBe(baseline.continued);
  expect(projected.finalReply).toBe(baseline.finalReply);
  expect(project(next)).toBe(projected);
  const thinking = foldLiveEvent(
    next,
    {
      type: 'REASONING_MESSAGE_CONTENT',
      threadId: 'agent',
      runId: 'live',
      messageId: 'thinking',
      delta: 'Inspecting',
    },
    'agent',
  );
  expect(project(thinking)).toBe(projected);
  expect(project(state).items).toEqual(baseline.items);
});

it('updates grouped failures and rebuilds ordering when a settled tool moves', () => {
  const project = createStreamTimeline();
  const baseline = history();
  const original = project(baseline);
  const changed = {
    ...baseline,
    toolCalls: baseline.toolCalls.map((tool) => ({ ...tool, status: 'failed', detail: 'Failure' })),
  };
  expect(project(changed).items[1]).toMatchObject({
    kind: 'tools',
    failed: true,
    items: [{ detail: 'Failure' }, { detail: 'Failure' }],
  });
  expect(original.items[1]).toMatchObject({ failed: false });
  const moved = {
    ...changed,
    toolCalls: [changed.toolCalls[0]!, { ...changed.toolCalls[1]!, order: 5 }],
  };
  expect(project(moved).items.map((item) => item.kind)).toEqual([
    'message',
    'tool',
    'message',
    'tool',
  ]);
});

it('refreshes author continuation and final-reply placement after history and identity changes', () => {
  const project = createStreamTimeline();
  const baseline = history();
  expect(project(baseline).continued.has('reply')).toBe(true);
  const changed = {
    ...baseline,
    messages: baseline.messages.map((message) =>
      message.id === 'reply' ? { ...message, role: 'user' as const } : message,
    ),
  };
  expect(project(changed).continued.has('reply')).toBe(false);
  expect(project(changed).previousRole).toBe('user');
  const recovered = {
    ...baseline,
    messages: [
      {
        id: 'recovered',
        role: 'assistant' as const,
        runId: 'live',
        content: 'Earlier',
        status: 'complete' as const,
        order: 0,
      },
      ...baseline.messages,
    ],
  };
  expect(project(recovered).items[0]).toMatchObject({ item: { id: 'recovered' } });
  expect(project(recovered).finalReply.get('live')).toBe('reply');
  const ephemeral = {
    ...recovered,
    messages: recovered.messages.map((message) =>
      message.id === 'reply' ? { ...message, liveOnly: true } : message,
    ),
  };
  expect(project(ephemeral).finalReply.get('live')).toBe('recovered');
});

it('updates pending-tool visibility, empty plans and recovery notices', () => {
  const project = createStreamTimeline();
  const state = {
    ...history(),
    activeRunId: 'live',
    toolCalls: [{ id: 'pending', runId: 'live', title: 'Work', status: 'running', order: 2 }],
    plans: [{ id: 'plan', entries: [], order: 3 }],
  };
  expect(project(state).items).toHaveLength(2);
  const stopped = {
    ...state,
    activeRunId: undefined,
    plans: [{ ...state.plans[0]!, entries: [{ title: 'Finish', status: 'pending' }] }],
    recovery: [{ id: 'notice', title: 'Recovered', detail: 'Loaded history', order: 5 }],
  };
  expect(project(stopped).items.map((item) => item.kind)).toEqual([
    'message',
    'tool',
    'plan',
    'message',
    'recovery',
  ]);
});
