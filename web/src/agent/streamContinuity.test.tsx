import { act, render } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { MessageStream } from './MessageStream';
import { conversationFromTranscript } from './conversation';
import { mergeTranscriptSnapshot } from './chronology';
import { foldV2Frame, foldV2Item } from './transport/projection';
import type { ConversationState } from './model';
import type { V2AgentStreamFrame } from '@chronoai/cma-sdk';

const agent = 'agt_stream';
const incarnation = 'a'.repeat(64);
const at = '2026-11-06T00:00:00Z';

function state(): ConversationState {
  return {
    ...conversationFromTranscript({ agentId: agent, throughSeq: 0, turns: [] }),
    agentIncarnation: incarnation,
    agentCursor: 'cursor:0',
  };
}

function durable(
  sequence: number,
  kind: unknown,
): Extract<V2AgentStreamFrame, { type: 'cma.agent.event' }> {
  return {
    type: 'cma.agent.event' as const,
    data: {
      agent_id: agent,
      incarnation,
      cursor: `cursor:${sequence}`,
      sequence,
      appended_at: at,
      kind,
      response_binding: null,
    },
  } as Extract<V2AgentStreamFrame, { type: 'cma.agent.event' }>;
}

function live(
  delta: string,
  messageId = 'live-answer',
): Extract<V2AgentStreamFrame, { type: 'cma.agent.live' }> {
  return {
    type: 'cma.agent.live' as const,
    data: {
      agent_id: agent,
      incarnation,
      turn_sequence: 1,
      after_sequence: 1,
      message_id: messageId,
      kind: 'text_delta' as const,
      delta,
    },
  };
}

function gap(): Extract<V2AgentStreamFrame, { type: 'cma.agent.live_gap' }> {
  return {
    type: 'cma.agent.live_gap' as const,
    data: {
      agent_id: agent,
      incarnation,
      turn_sequence: 1,
      frames: 1,
      reason: 'lagged' as const,
    },
  };
}

it('keeps visible streaming text monotonic and ignores every post-gap fragment', () => {
  let current = foldV2Item(state(), durable(1, { type: 'turn_started', turn_sequence: 1 }).data);
  const lengths: number[] = [];
  current = foldV2Frame(current, live('first'), agent);
  lengths.push(current.messages[0]?.content.length ?? 0);
  current = foldV2Frame(current, gap(), agent);
  lengths.push(current.messages[0]?.content.length ?? 0);
  current = foldV2Frame(current, live('hole'), agent);
  lengths.push(current.messages[0]?.content.length ?? 0);
  current = foldV2Frame(current, gap(), agent);
  lengths.push(current.messages[0]?.content.length ?? 0);
  expect(lengths).toEqual([5, 5, 5, 5]);
  expect(current.messages[0]?.content).toBe('first');
  expect(current.messages[0]?.content).not.toBe('');
});

it('replaces a live reply with its durable message without remounting the article', () => {
  const initial = state();
  const liveState = foldV2Frame(
    foldV2Item(initial, durable(1, { type: 'turn_started', turn_sequence: 1 }).data),
    live('partial'),
    agent,
  );
  const view = render(<MessageStream conversation={liveState} onApproval={vi.fn()} />);
  const node = view.container.querySelector('[data-message-key]');
  expect(node).not.toBeNull();
  let completed = foldV2Item(
    liveState,
    durable(2, { type: 'message', role: 'assistant', content: 'complete answer' }).data,
  );
  completed = foldV2Item(completed, durable(3, { type: 'turn_completed', turn_sequence: 1 }).data);
  act(() => view.rerender(<MessageStream conversation={completed} onApproval={vi.fn()} />));
  expect(view.container.querySelector('[data-message-key]')).toBe(node);
  expect(view.container.querySelector('[data-message-key]')!).toHaveTextContent('complete answer');
});

it('keeps multiple assistant messages unique and stable across durable replacement', () => {
  let current = foldV2Item(state(), durable(1, { type: 'turn_started', turn_sequence: 1 }).data);
  current = foldV2Frame(current, live('first', 'live-first'), agent);
  current = foldV2Frame(current, live('second', 'live-second'), agent);
  current = foldV2Item(
    current,
    durable(2, {
      type: 'tool_started',
      call_id: 'tool',
      title: 'Inspect',
      kind: 'read',
      status: 'running',
    }).data,
  );
  const errors = vi.spyOn(console, 'error').mockImplementation(() => {});
  const view = render(<MessageStream conversation={current} onApproval={vi.fn()} />);
  const before = [...view.container.querySelectorAll<HTMLElement>('[data-message-key]')];
  current = foldV2Item(
    current,
    durable(3, { type: 'message', role: 'assistant', content: 'first durable' }).data,
  );
  current = foldV2Item(
    current,
    durable(4, { type: 'message', role: 'assistant', content: 'second durable' }).data,
  );
  act(() => view.rerender(<MessageStream conversation={current} onApproval={vi.fn()} />));
  const after = [...view.container.querySelectorAll<HTMLElement>('[data-message-key]')];
  expect(after).toHaveLength(2);
  expect(after[0]).toBe(before[0]);
  expect(after[1]).toBe(before[1]);
  expect(
    [...view.container.querySelectorAll('.agent-content')].map((node) => node.textContent),
  ).toEqual(['first durable', 'second durable']);
  expect(errors.mock.calls.some(([message]) => String(message).includes('unique "key"'))).toBe(
    false,
  );
  errors.mockRestore();
});

it('clears the fixed live recovery slot when durable completion is recorded', () => {
  let current = foldV2Frame(
    foldV2Item(state(), durable(1, { type: 'turn_started', turn_sequence: 1 }).data),
    live('partial'),
    agent,
  );
  current = foldV2Frame(current, gap(), agent);
  const view = render(<MessageStream conversation={current} onApproval={vi.fn()} />);
  expect(view.container.querySelector('.message-stream__live-recovery-slot')).toHaveTextContent(
    'Refreshing live updates…',
  );
  current = foldV2Item(
    current,
    durable(2, { type: 'message', role: 'assistant', content: 'complete answer' }).data,
  );
  current = foldV2Item(current, durable(3, { type: 'turn_completed', turn_sequence: 1 }).data);
  act(() => view.rerender(<MessageStream conversation={current} onApproval={vi.fn()} />));
  expect(view.container.querySelector('.message-stream__live-recovery-slot')).toBeEmptyDOMElement();
  expect(view.container.querySelector('.active-thinking[data-presence="visible"]')).toBeNull();
  expect(view.container.querySelector('[data-streaming]')).toBeNull();
});

it('does not unfreeze the second message when a snapshot contains the first message', () => {
  let current = foldV2Item(state(), durable(1, { type: 'turn_started', turn_sequence: 1 }).data);
  current = foldV2Item(
    current,
    durable(2, { type: 'message', role: 'assistant', content: 'first durable' }).data,
  );
  const second = live('second partial', 'live-second');
  current = foldV2Frame(current, { ...second, data: { ...second.data, after_sequence: 2 } }, agent);
  current = foldV2Frame(current, gap(), agent);
  let snapshot = foldV2Item(state(), durable(1, { type: 'turn_started', turn_sequence: 1 }).data);
  snapshot = foldV2Item(
    snapshot,
    durable(2, { type: 'message', role: 'assistant', content: 'first durable' }).data,
  );
  const merged = mergeTranscriptSnapshot(current, snapshot);
  expect(merged.messages.map((message) => message.content)).toEqual([
    'first durable',
    'second partial',
  ]);
  expect(merged.liveGapRuns?.has('1')).toBe(true);
  expect(merged.recovery.some((notice) => notice.clearsOn !== undefined)).toBe(true);
});

it('keeps the same reply DOM node through snapshot replacement and repeated snapshots', () => {
  let current = foldV2Item(state(), durable(1, { type: 'turn_started', turn_sequence: 1 }).data);
  current = foldV2Frame(current, live('partial'), agent);
  const view = render(<MessageStream conversation={current} onApproval={vi.fn()} />);
  const node = view.container.querySelector('[data-message-key]');
  expect(node).not.toBeNull();
  let snapshot = foldV2Item(state(), durable(1, { type: 'turn_started', turn_sequence: 1 }).data);
  snapshot = foldV2Item(
    snapshot,
    durable(2, { type: 'message', role: 'assistant', content: 'partial completed' }).data,
  );
  for (let pass = 0; pass < 2; pass += 1) {
    current = mergeTranscriptSnapshot(current, snapshot);
    view.rerender(<MessageStream conversation={current} onApproval={vi.fn()} />);
    expect(view.container.querySelector('[data-message-key]')).toBe(node);
    expect(node).toHaveTextContent('partial completed');
  }
});

it('replaces a frozen second reply with authoritative snapshot text even when its prefix differs', () => {
  let snapshot = foldV2Item(state(), durable(1, { type: 'turn_started', turn_sequence: 1 }).data);
  snapshot = foldV2Item(
    snapshot,
    durable(2, { type: 'message', role: 'assistant', content: 'first durable' }).data,
  );
  const second = live('second partial', 'live-second');
  let current = foldV2Frame(
    snapshot,
    { ...second, data: { ...second.data, after_sequence: 2 } },
    agent,
  );
  current = foldV2Frame(current, gap(), agent);
  const view = render(<MessageStream conversation={current} onApproval={vi.fn()} />);
  const nodes = [...view.container.querySelectorAll('[data-message-key]')];
  expect(nodes).toHaveLength(2);
  current = mergeTranscriptSnapshot(current, snapshot);
  expect(current.liveGapRuns?.has('1')).toBe(true);
  snapshot = foldV2Item(
    snapshot,
    durable(3, { type: 'message', role: 'assistant', content: 'authoritative second answer' }).data,
  );
  current = mergeTranscriptSnapshot(current, snapshot);
  view.rerender(<MessageStream conversation={current} onApproval={vi.fn()} />);
  const replies = [...view.container.querySelectorAll('[data-message-key]')];
  expect(replies).toHaveLength(2);
  expect(replies[0]).toBe(nodes[0]);
  expect(replies[1]).toBe(nodes[1]);
  expect(replies[1]).toHaveTextContent('authoritative second answer');
  expect(view.container).not.toHaveTextContent('second partial');
  expect(current.liveGapRuns?.has('1')).not.toBe(true);
  expect(current.recovery.some((notice) => notice.clearsOn !== undefined)).toBe(false);
});
