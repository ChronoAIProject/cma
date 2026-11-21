import { act, render } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { MessageStream } from './MessageStream';
import { conversationFromTranscript, foldLiveEvent } from './conversation';
import type { ConversationMessage, ConversationState } from './model';
import type { OptimisticMessage } from './optimisticMessages';
import { observeAnimations } from '../test/animations';

let motion: ReturnType<typeof observeAnimations>;
beforeEach(() => {
  motion = observeAnimations();
});
afterEach(() => motion.restore());
const empty = () => conversationFromTranscript({ agentId: 'agent', throughSeq: 0, turns: [] });
const recorded = (id: string): ConversationMessage => ({
  id,
  runId: 'run',
  role: 'assistant',
  content: 'Recorded reply',
  status: 'complete',
  sequence: 10,
});
function stream(
  conversation: ConversationState,
  optimisticMessages: readonly OptimisticMessage[] = [],
  arrivalsEnabled = true,
) {
  return (
    <MessageStream
      conversation={conversation}
      optimisticMessages={optimisticMessages}
      arrivalsEnabled={arrivalsEnabled}
      onApproval={vi.fn()}
    />
  );
}
function live(conversation: ConversationState, id = 'reply', delta = 'New reply') {
  return [
    { type: 'RUN_STARTED' },
    { type: 'TEXT_MESSAGE_START', messageId: id, role: 'assistant' },
    { type: 'TEXT_MESSAGE_CONTENT', messageId: id, delta },
  ].reduce(
    (state, event) => foldLiveEvent(state, { ...event, threadId: 'agent', runId: 'run' }, 'agent'),
    conversation,
  );
}

it('fades a new outgoing preview once and never repeats its fade for acceptance or replay', () => {
  const baseline = { ...empty(), messages: [recorded('history')] };
  const { rerender, container } = render(stream(baseline));
  const message: OptimisticMessage = { id: 'local-one', state: 'sending', content: 'New question' };
  rerender(stream(baseline, [message]));
  expect(motion.records).toHaveLength(1);
  const outgoing = container.querySelector('[data-message-key="local:local-one"]');
  expect(motion.records[0]?.element).toBe(outgoing);
  expect(motion.records[0]?.frames).toEqual([{ opacity: 0 }, { opacity: 1 }]);
  expect(motion.records[0]?.options).toMatchObject({ duration: 200 });
  rerender(stream(baseline, [{ ...message, state: 'accepted', receipt: { runId: 'run' } }]));
  expect(container.querySelector('[data-message-key="local:local-one"]')).toBe(outgoing);
  expect(motion.records).toHaveLength(1);
  expect(motion.records[0]?.animation.cancel).not.toHaveBeenCalled();
  rerender(
    stream({
      ...baseline,
      messages: [
        ...baseline.messages,
        { ...recorded('accepted'), role: 'user', content: message.content },
      ],
    }),
  );
  expect(motion.records).toHaveLength(1);
  rerender(stream(baseline, [{ ...message, id: 'recovered', state: 'unconfirmed' }]));
  expect(motion.records).toHaveLength(1);
});

it('fades a new live reply but preserves its node and animation while more tokens arrive', () => {
  const baseline = { ...empty(), messages: [recorded('history')] };
  const { rerender, container } = render(stream(baseline));
  const started = live(baseline);
  rerender(stream(started));
  expect(motion.records).toHaveLength(1);
  const reply = container.querySelector('[data-message-key="message:reply"]');
  expect(motion.records[0]?.element).toBe(reply);
  const continued = foldLiveEvent(
    started,
    {
      type: 'TEXT_MESSAGE_CONTENT',
      threadId: 'agent',
      runId: 'run',
      messageId: 'reply',
      delta: ' continues',
    },
    'agent',
  );
  rerender(stream(continued));
  expect(container.querySelector('[data-message-key="message:reply"]')).toBe(reply);
  expect(reply).toHaveTextContent('New reply continues');
  expect(motion.records).toHaveLength(1);
  expect(motion.records[0]?.animation.cancel).not.toHaveBeenCalled();
  rerender(stream({ ...baseline, messages: [...baseline.messages, recorded('durable-reply')] }));
  expect(motion.records).toHaveLength(1);
});

it('treats initial live content, paged history, recovered previews and disabled arrivals as a baseline', () => {
  const initial = live(empty(), 'already-running');
  const { rerender } = render(stream(initial));
  rerender(stream({ ...initial, messages: [...initial.messages, recorded('older-page')] }));
  rerender(stream(live(initial, 'loading-row'), [], false));
  rerender(stream(live(initial, 'loading-row')));
  rerender(stream(initial, [{ id: 'accepted-journal', state: 'accepted', content: 'Recovered' }]));
  expect(motion.records).toHaveLength(0);
});

it('cancels active row motion when reduced motion changes and does not replay skipped arrivals', () => {
  const { rerender, unmount } = render(stream(empty()));
  rerender(stream(live(empty())));
  act(() => motion.reduce(true));
  expect(motion.records[0]?.animation.cancel).toHaveBeenCalledOnce();
  rerender(stream(live(empty(), 'reduced')));
  act(() => motion.reduce(false));
  rerender(stream(live(empty(), 'reduced')));
  expect(motion.records).toHaveLength(1);
  rerender(stream(live(empty(), 'next')));
  expect(motion.records).toHaveLength(2);
  unmount();
  expect(motion.records[1]?.animation.cancel).toHaveBeenCalledOnce();
});

it('does not replay new hidden-portal rows when that portal becomes visible', () => {
  const draw = (conversation: ConversationState, hidden: boolean) => (
    <div hidden={hidden}>{stream(conversation)}</div>
  );
  const { rerender } = render(draw(empty(), true));
  const incoming = live(empty());
  rerender(draw(incoming, true));
  rerender(draw(incoming, false));
  expect(motion.records).toHaveLength(0);
});
