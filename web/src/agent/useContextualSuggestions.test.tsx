import { act, cleanup, renderHook } from '@testing-library/react';
import type { ReactNode } from 'react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { SuggestedReplyReading, SuggestedReplySet } from '@cma/client';
import type { CmaWebClient } from '../api/client';
import { CmaClientProvider } from '../api/context';
import { agentClient } from '../test/agent';
import type { ConversationState } from './conversation';
import { useContextualSuggestions } from './useContextualSuggestions';

const conversation: ConversationState = {
  cursor: 6,
  latestTurnId: '3',
  messages: [
    {
      id: 'event:5',
      runId: '3',
      sequence: 5,
      role: 'assistant',
      content: 'SQLite suits a local-only app.',
      status: 'complete',
      finalAnswer: true,
    },
  ],
  toolCalls: [],
  reasoning: [],
  approvals: [],
  plans: [],
  recovery: [],
};
const set: SuggestedReplySet = {
  id: 'overdue-suggestion',
  viewerId: 'owner',
  agentId: 'agt_overdue',
  answerId: 'event:5',
  scope: { turn_sequence: 3, answer_sequence: 5 },
  source: { kind: 'generated', version: 'contextual-v3' },
  items: [{ id: 'overdue-suggestion:0', text: 'Explain SQLite backup options.', rank: 0 }],
};
const running: SuggestedReplyReading = { status: 'running', set: null, observation: null };
const ready: SuggestedReplyReading = { status: 'ready', set, observation: null };
type Read = NonNullable<CmaWebClient['readSuggestedReplies']>;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(0);
  vi.spyOn(document, 'hidden', 'get').mockReturnValue(false);
  vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('visible');
});
afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});
const advance = (ms: number) => act(() => vi.advanceTimersByTimeAsync(ms));
const hints = () =>
  act(() => {
    window.dispatchEvent(new Event('focus'));
    document.dispatchEvent(new Event('visibilitychange'));
  });

function setup(read: Read) {
  const client = agentClient({ readSuggestedReplies: read });
  const wrapper = ({ children }: { children: ReactNode }) => (
    <CmaClientProvider client={client}>{children}</CmaClientProvider>
  );
  return renderHook(
    ({ state, enabled }) => useContextualSuggestions(state, 'owner', 'agt_overdue', enabled),
    { initialProps: { state: conversation, enabled: true }, wrapper },
  );
}

it('observes deadline-past Running then Ready in the existing scope within its budget', async () => {
  const read = vi.fn<Read>(async () => (Date.now() < 71_000 ? running : ready));
  const view = setup(read);
  await advance(70_500);
  expect(read).toHaveBeenCalledTimes(48);
  expect(view.result.current).toBeUndefined();
  view.rerender({ state: { ...conversation }, enabled: true });
  hints();
  await advance(500);
  expect(read).toHaveBeenCalledTimes(48);
  await advance(1000);
  expect(read).toHaveBeenCalledTimes(49);
  expect(view.result.current).toEqual(set);
  expect(read.mock.calls.every(([, scope]) => scope.answer_sequence === 5)).toBe(true);
  hints();
  await advance(100_000);
  expect(read).toHaveBeenCalledTimes(49);
  expect(view.result.current).toEqual(set);
});

it.each(['interrupted', 'unavailable', 'timeout'] as const)(
  'stops on durable %s failure despite same-scope rerenders and foreground hints',
  async (failure) => {
    const failed: SuggestedReplyReading = {
      status: 'failed',
      set: null,
      observation: {
        elapsed_ms: 71_000,
        failure,
        usage: {
          model: null,
          input_tokens: null,
          cached_input_tokens: null,
          output_tokens: null,
          cost: null,
        },
      },
    };
    const read = vi.fn<Read>().mockResolvedValue(failed);
    const view = setup(read);
    await advance(0);
    view.rerender({ state: { ...conversation }, enabled: true });
    for (let index = 0; index < 80; index++) {
      hints();
      await advance(1500);
    }
    expect(read).toHaveBeenCalledOnce();
    expect(view.result.current).toBeUndefined();
  },
);

it('starts a new bounded observation only after explicit disable/enable or remount', async () => {
  const read = vi.fn<Read>().mockResolvedValue({ status: 'failed', set: null });
  const view = setup(read);
  await advance(0);
  view.rerender({ state: conversation, enabled: false });
  await advance(0);
  view.rerender({ state: conversation, enabled: true });
  await advance(0);
  expect(read).toHaveBeenCalledTimes(2);
  view.unmount();
  setup(read);
  await advance(0);
  expect(read).toHaveBeenCalledTimes(3);
  hints();
  await advance(100_000);
  expect(read).toHaveBeenCalledTimes(3);
});

it('stops after 50 reads even when foreground hints continue', async () => {
  const read = vi.fn<Read>().mockResolvedValue(running);
  const view = setup(read);
  await advance(73_500);
  expect(read).toHaveBeenCalledTimes(50);
  expect(view.result.current).toBeUndefined();
  read.mockResolvedValue(ready);
  hints();
  await advance(100_000);
  expect(read).toHaveBeenCalledTimes(50);
  expect(view.result.current).toBeUndefined();
});

it('aborts at the 75-second wall deadline and ignores a late response below the read limit', async () => {
  let finish!: (value: SuggestedReplyReading) => void;
  const read = vi.fn<Read>().mockImplementation(() => new Promise((resolve) => (finish = resolve)));
  const view = setup(read);
  await advance(74_999);
  const signal = read.mock.calls[0]![2]!.signal!;
  expect(signal.aborted).toBe(false);
  await advance(1);
  expect(signal.aborted).toBe(true);
  await act(async () => finish(ready));
  hints();
  await advance(100_000);
  expect(read).toHaveBeenCalledOnce();
  expect(view.result.current).toBeUndefined();
});

it('pauses hidden reads while preserving the wall deadline', async () => {
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
  const read = vi.fn<Read>().mockResolvedValue(running);
  setup(read);
  await advance(70_000);
  expect(read).not.toHaveBeenCalled();
  hidden.mockReturnValue(false);
  hints();
  await advance(0);
  expect(read).toHaveBeenCalledOnce();
  hidden.mockReturnValue(true);
  hints();
  await advance(5000);
  hidden.mockReturnValue(false);
  hints();
  await advance(100_000);
  expect(read).toHaveBeenCalledOnce();
});

it('ignores an old scope response after the replacement scope already has suggestions', async () => {
  let finish!: (value: SuggestedReplyReading) => void;
  const nextState: ConversationState = {
    ...conversation,
    latestTurnId: '7',
    messages: [{ ...conversation.messages[0]!, id: 'event:9', runId: '7', sequence: 9 }],
  };
  const nextSet = {
    ...set,
    id: 'next-suggestion',
    answerId: 'event:9',
    scope: { turn_sequence: 7, answer_sequence: 9 },
  };
  const read = vi
    .fn<Read>()
    .mockImplementationOnce(() => new Promise((resolve) => (finish = resolve)))
    .mockResolvedValue({ ...ready, set: nextSet });
  const view = setup(read);
  await advance(0);
  const oldSignal = read.mock.calls[0]![2]!.signal!;
  view.rerender({ state: nextState, enabled: true });
  await advance(0);
  expect(oldSignal.aborted).toBe(true);
  expect(view.result.current).toEqual(nextSet);
  await act(async () => finish(ready));
  expect(view.result.current).toEqual(nextSet);
});

it('hides a cached old tuple immediately when the answer scope changes', async () => {
  const read = vi.fn<Read>().mockResolvedValue(ready);
  const view = setup(read);
  await advance(0);
  expect(view.result.current).toEqual(set);
  read.mockImplementation(() => new Promise(() => {}));
  view.rerender({
    state: {
      ...conversation,
      latestTurnId: '7',
      messages: [{ ...conversation.messages[0]!, id: 'event:9', runId: '7', sequence: 9 }],
    },
    enabled: true,
  });
  expect(view.result.current).toBeUndefined();
  await advance(0);
  expect(view.result.current).toBeUndefined();
});
