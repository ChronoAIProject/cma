import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { AGENT_DETAIL } from '../../test/agent';
import { BOOTSTRAP_AGENT, bootstrapSandbox } from '../../test/bootstrap';
import { sandboxFixture } from '../../test/hierarchy';
import { useAgentController } from '../controller';
import { cursor, harness, incarnation, item, json, page } from './controller.fixture';
import { agentTransportJournal } from './journal';
import { queuedObservation } from './queued.fixture';

beforeEach(() => sessionStorage.clear());
afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

it('retains bootstrap admission through runtime binding, snapshot refresh, tail and reload without resending', async () => {
  vi.useFakeTimers();
  const h = harness();
  const detail = vi.spyOn(h.client, 'readAgent').mockResolvedValue(BOOTSTRAP_AGENT);
  const parent = vi.spyOn(h.client, 'readSandbox').mockResolvedValue(bootstrapSandbox());
  const created = item(1, { type: 'created', workspace_id: 'wsp_1', worktree_id: 'wkt_1' });
  h.snapshot.mockImplementation(async () => json(page([created], 1)));
  h.observeQueued.mockImplementation(async () => json(queuedObservation()));
  const view = renderHook(() => useAgentController(h.client, 'agt_1', { viewerId: 'alice' }));
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(view.result.current.inputAvailability.allowed).toBe(true);
  expect(view.result.current.resourceAvailability.state).toBe('inactive');
  const onAccepted = vi.fn();
  const input = 'Continue after the original first message';
  await act(() => view.result.current.send(input, onAccepted));
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
  const retained = journal.read();
  expect(retained).toMatchObject({ state: 'accepted', input });
  expect(view.result.current.queuedMessage?.message).toBe(input);

  // Runtime and conversation binding advance the same durable Agent log. The
  // server's cursor incarnation depends only on Agent ID and created_at.
  const binding = { response_id: 'resp_1', command_id: 'cmd_1', turn_sequence: 7 };
  const prepared = [
    created,
    item(2, { type: 'conversation_available' }),
    item(3, { type: 'turn_started', turn_sequence: 3 }),
    item(4, { type: 'message', role: 'user', content: 'Original first message' }),
    item(5, { type: 'message', role: 'assistant', content: 'Original work completed' }),
    item(6, { type: 'turn_completed', turn_sequence: 3 }),
    item(7, { type: 'turn_started', turn_sequence: 7 }, binding),
    item(8, { type: 'message', role: 'user', content: input }),
  ];
  detail.mockResolvedValue(AGENT_DETAIL);
  parent.mockResolvedValue(sandboxFixture());
  h.snapshot.mockImplementation(async () => json(page(prepared, 8)));
  h.observeQueued.mockImplementation(async () => json(queuedObservation('resp_1', 8, false)));
  await act(() => vi.advanceTimersByTimeAsync(1500));
  expect(view.result.current.state).toMatchObject({
    detail: { status: 'ready' },
    conversation: {
      agentIncarnation: incarnation,
      agentCursor: cursor(8),
      messages: [
        expect.objectContaining({ content: 'Original first message' }),
        expect.objectContaining({ content: 'Original work completed' }),
        expect.objectContaining({ content: input }),
      ],
      responseBinding: { responseId: 'resp_1', commandId: 'cmd_1', turnSequence: 7 },
    },
  });
  expect(view.result.current.acceptedMessage).toBe(input);
  expect(view.result.current.actionError).toBeUndefined();
  expect(journal.read()).toEqual(retained);
  const streams = () => h.fetch.mock.calls.filter(([url]) => String(url).includes('/events?'));
  expect(
    new URL(String(streams().at(-1)![0]), 'https://cma.example').searchParams.get('after_cursor'),
  ).toBe(cursor(8));

  await act(async () => {
    h.durable(9, { type: 'message', role: 'assistant', content: 'Queued work completed' });
    h.durable(10, { type: 'turn_completed', turn_sequence: 7 });
  });
  const complete = [
    ...prepared,
    item(9, { type: 'message', role: 'assistant', content: 'Queued work completed' }),
    item(10, { type: 'turn_completed', turn_sequence: 7 }),
  ];
  h.snapshot.mockImplementation(async () => json(page(complete, 10)));
  act(() => view.result.current.reload());
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(view.result.current.state).toMatchObject({
    conversation: {
      agentIncarnation: incarnation,
      agentCursor: cursor(10),
      messages: [
        expect.objectContaining({ content: 'Original first message' }),
        expect.objectContaining({ content: 'Original work completed' }),
        expect.objectContaining({ content: input }),
        expect.objectContaining({ content: 'Queued work completed' }),
      ],
    },
  });
  expect(journal.read()).toEqual(retained);
  expect(onAccepted).toHaveBeenCalledOnce();
  expect(h.post).toHaveBeenCalledOnce();
  expect(h.client.runAgent).not.toHaveBeenCalled();
  expect(view.result.current.actionError).toBeUndefined();
  view.unmount();
});
