import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { AGENT_USAGE } from '../test/agent';
import { useAgentController } from './controller';
import {
  at,
  binding,
  cursor,
  harness,
  incarnation,
  item,
  json,
  page,
} from './transport/controller.fixture';

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((done) => (resolve = done));
  return { promise, resolve };
}

const head = (through = 0, active: unknown = null) => ({
  object: 'cma.agent.event_head',
  schema_version: 1,
  agent_id: 'agt_1',
  incarnation,
  through_cursor: cursor(through),
  active_turn: active,
});
const transcript = { agentId: 'agt_1', throughSeq: 0, turns: [] };

beforeEach(() => sessionStorage.clear());
afterEach(() => vi.restoreAllMocks());

it.each(['active', 'completed'] as const)(
  'shows new live Thinking and replies while all history and usage are held; preserves %s output on hydration',
  async (outcome) => {
    const h = harness();
    h.head.mockResolvedValue(json(head()));
    const history = deferred<Response>();
    const text = deferred<typeof transcript>();
    const usage = deferred<typeof AGENT_USAGE>();
    h.snapshot.mockReturnValueOnce(history.promise);
    vi.spyOn(h.client, 'readAgentTranscript').mockReturnValueOnce(text.promise);
    vi.spyOn(h.client, 'readAgentUsage').mockReturnValueOnce(usage.promise);
    const view = renderHook(() => useAgentController(h.client, 'agt_1'));
    await waitFor(() => expect(h.streaming).toBe(true));
    expect(view.result.current.state).toMatchObject({ historyLoading: true });
    await waitFor(() => expect(view.result.current.inputAvailability.allowed).toBe(true));
    const accepted = vi.fn();
    await act(async () => view.result.current.send('New question while loading', accepted));
    expect(accepted).toHaveBeenCalledOnce();
    await act(async () => {
      h.durable(1, { type: 'turn_started', turn_sequence: 1 }, binding);
      h.durable(2, { type: 'message', role: 'user', content: 'New question while loading' });
      h.live('reasoning_summary_delta', 'Thinking now', 2);
      h.live('text_delta', 'Live answer', 2);
    });
    await waitFor(() =>
      expect(view.result.current.state).toMatchObject({
        historyLoading: true,
        conversation: {
          activeRunId: '1',
          responseBinding: { responseId: 'resp_1', commandId: 'cmd_1', turnSequence: 1 },
          reasoning: [expect.objectContaining({ content: 'Thinking now' })],
          messages: [
            expect.objectContaining({ content: 'New question while loading' }),
            expect.objectContaining({ content: 'Live answer' }),
          ],
        },
      }),
    );
    if (outcome === 'completed') {
      await act(async () => {
        h.durable(3, { type: 'message', role: 'assistant', content: 'Live answer complete' });
        h.durable(4, { type: 'turn_completed', turn_sequence: 1 });
      });
      await waitFor(() =>
        expect(view.result.current.state).toMatchObject({
          historyLoading: true,
          conversation: { activeRunId: undefined },
        }),
      );
    }
    await act(async () => {
      text.resolve(transcript);
      usage.resolve(AGENT_USAGE);
      history.resolve(json(page([], 0)));
    });
    await waitFor(() => expect(view.result.current.state).toMatchObject({ historyLoading: false }));
    const state = view.result.current.state;
    if (state.status !== 'ready') throw new Error('Expected loaded Agent');
    expect(state.conversation.messages.map((message) => message.content)).toEqual([
      'New question while loading',
      outcome === 'active' ? 'Live answer' : 'Live answer complete',
    ]);
    expect(state.conversation.activeRunId).toBe(outcome === 'active' ? '1' : undefined);
    expect(state.conversation.reasoning.map((block) => block.content)).toEqual(
      outcome === 'active' ? ['Thinking now'] : [],
    );
    expect(h.post).toHaveBeenCalledOnce();
    expect(h.fetch.mock.calls.filter(([url]) => String(url).includes('/events?'))).toHaveLength(1);
    view.unmount();
  },
);

it('uses the current active turn without replaying historical starts and refreshes an older snapshot prefix', async () => {
  const h = harness();
  const start = item(1, { type: 'turn_started', turn_sequence: 1 }, binding);
  h.head.mockResolvedValue(json(head(2, start)));
  const history = deferred<Response>();
  h.snapshot
    .mockReturnValueOnce(history.promise)
    .mockResolvedValueOnce(
      json(
        page([start, item(2, { type: 'message', role: 'user', content: 'Already running' })], 2),
      ),
    );
  vi.spyOn(h.client, 'readAgentTranscript').mockResolvedValue({
    agentId: 'agt_1',
    throughSeq: 2,
    turns: [
      {
        turnSequence: 1,
        startedAt: at,
        ownerMessage: 'Already running',
        agentMessages: [],
        messages: [{ sequence: 2, role: 'user', content: 'Already running', appendedAt: at }],
        outcome: { type: 'active' },
      },
    ],
  });
  const view = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.streaming).toBe(true));
  await act(async () => {
    h.live('reasoning_summary_delta', 'Current thoughts', 2);
  });
  await waitFor(() =>
    expect(view.result.current.state).toMatchObject({
      historyLoading: true,
      conversation: { reasoning: [expect.objectContaining({ content: 'Current thoughts' })] },
    }),
  );
  await act(async () => history.resolve(json(page([], 0))));
  await waitFor(() => expect(view.result.current.state).toMatchObject({ historyLoading: false }));
  expect(h.snapshot).toHaveBeenCalledTimes(2);
  expect(view.result.current.state).toMatchObject({
    conversation: {
      activeRunId: '1',
      reasoning: [expect.objectContaining({ content: 'Current thoughts' })],
      messages: [expect.objectContaining({ content: 'Already running' })],
    },
  });
  view.unmount();
});

it('aborts early live replay when account authority changes while history is held', async () => {
  const h = harness();
  h.head.mockResolvedValue(json(head()));
  const history = deferred<Response>();
  h.snapshot.mockReturnValueOnce(history.promise);
  const view = renderHook(
    ({ viewer }) => useAgentController(h.client, 'agt_1', { viewerId: viewer }),
    { initialProps: { viewer: 'alice' } },
  );
  await waitFor(() => expect(h.streaming).toBe(true));
  h.currentUser.mockResolvedValue('bob');
  view.rerender({ viewer: 'bob' });
  await act(async () => history.resolve(json(page([], 0))));
  await waitFor(() => expect(h.cancel).toHaveBeenCalled());
  expect(h.post).not.toHaveBeenCalled();
  view.unmount();
});
