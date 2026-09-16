import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ProtocolError, TransportError } from '@chronoai/cma-sdk';
import { agentTransportJournal } from './journal';
import { IdentityChangedError, isIdentityChanged } from './identity';
import { accepted, cursor, harness, item, json, page } from './controller.fixture';
import { loadV2Agent } from './load';
import { followV2Agent } from './replay';
import { act, renderHook, waitFor } from '@testing-library/react';
import { useAgentController } from '../controller';

beforeEach(() => sessionStorage.clear());
afterEach(() => vi.useRealTimers());

async function opened() {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  return { ...h, session };
}

it('recognizes identity failures wrapped by stream callbacks without looping over causes', () => {
  expect(
    isIdentityChanged(
      new ProtocolError('callback failed', {
        cause: new IdentityChangedError(),
      }),
    ),
  ).toBe(true);
  const cycle = new Error('cycle');
  cycle.cause = cycle;
  expect(isIdentityChanged(cycle)).toBe(false);
  expect(isIdentityChanged('sign-in changed')).toBe(false);
});

it('refuses a transport selection if identity changes while eligibility is loading', async () => {
  const h = harness();
  h.currentUser.mockResolvedValueOnce('alice').mockResolvedValue('bob');
  await expect(h.client.agentTransport!.open('agt_1')).rejects.toBeInstanceOf(IdentityChangedError);
  expect(sessionStorage.length).toBe(0);
  expect(h.snapshot).not.toHaveBeenCalled();
});

it.each(['bob', undefined])(
  'invalidates a retained session after account becomes %s',
  async (user) => {
    const h = await opened();
    h.session.journal.retain('Alice private draft');
    h.currentUser.mockResolvedValue(user);
    await expect(h.session.send('Alice private draft', vi.fn())).rejects.toBeInstanceOf(
      IdentityChangedError,
    );
    expect(h.post).not.toHaveBeenCalled();
    expect(sessionStorage.length).toBe(0);
    expect(() => h.session.journal.read()).toThrow(IdentityChangedError);
  },
);

it('does not let an old account check erase a newly selected account journal', async () => {
  const h = await opened();
  let finish!: (user: string) => void;
  h.currentUser.mockReturnValueOnce(
    new Promise((resolve) => {
      finish = resolve;
    }),
  );
  const authorized = h.session.authorize();
  const rejected = expect(authorized).rejects.toBeInstanceOf(IdentityChangedError);
  const bob = agentTransportJournal(sessionStorage, 'bob', 'agt_1');
  bob.select(true);
  const pending = bob.retain('Bob retained message');
  finish('bob');
  await rejected;
  expect(bob.read()).toEqual(pending);
  expect(() => h.session.journal.invalidate()).toThrow(IdentityChangedError);
  expect(bob.read()).toEqual(pending);
});

it('does not acknowledge an input receipt delivered after the account changes', async () => {
  const h = await opened();
  h.post.mockImplementationOnce(async () => {
    h.currentUser.mockResolvedValue('bob');
    return accepted();
  });
  const onAccepted = vi.fn();
  const failure = await h.session.send('Alice input', onAccepted).catch((error: unknown) => error);
  expect(isIdentityChanged(failure)).toBe(true);
  expect(onAccepted).not.toHaveBeenCalled();
  expect(sessionStorage.length).toBe(0);
});

it('rejects a snapshot page before releasing any projection under a changed account', async () => {
  const h = await opened();
  const progress = vi.fn();
  h.snapshot.mockImplementationOnce(async () => {
    h.currentUser.mockResolvedValue('bob');
    return json(page([item(1, { type: 'turn_started', turn_sequence: 1 })], 2, 1));
  });
  await expect(
    loadV2Agent(h.client, 'agt_1', new AbortController().signal, h.session, true, progress),
  ).rejects.toBeInstanceOf(IdentityChangedError);
  expect(progress).not.toHaveBeenCalled();
  expect(h.snapshot).toHaveBeenCalledOnce();
});

it('checks identity again before releasing the final snapshot page', async () => {
  const h = await opened();
  const progress = vi.fn();
  h.snapshot
    .mockResolvedValueOnce(json(page([item(1, { type: 'turn_started', turn_sequence: 1 })], 2, 1)))
    .mockImplementationOnce(async () => {
      h.currentUser.mockResolvedValue('bob');
      return json(page([item(2, { type: 'message', role: 'assistant', content: 'Bob text' })], 2));
    });
  await expect(
    loadV2Agent(h.client, 'agt_1', new AbortController().signal, h.session, true, progress),
  ).rejects.toBeInstanceOf(IdentityChangedError);
  expect(progress).toHaveBeenCalledOnce();
  expect(progress.mock.calls[0]![0].conversation.messages).toEqual([]);
});

it('does not release incomplete history after a transient read crosses accounts', async () => {
  const h = await opened();
  h.snapshot.mockImplementationOnce(async () => {
    h.currentUser.mockResolvedValue('bob');
    throw new TypeError('connection lost');
  });
  await expect(
    loadV2Agent(h.client, 'agt_1', new AbortController().signal, h.session, true),
  ).rejects.toBeInstanceOf(IdentityChangedError);
});

it('revalidates account identity before reconnecting an otherwise retryable Agent tail', async () => {
  vi.useFakeTimers();
  const h = await opened();
  const events = vi.spyOn(h.session.events, 'events').mockImplementationOnce(async () => {
    h.currentUser.mockResolvedValue('bob');
    throw new TransportError('connection lost');
  });
  const onEvent = vi.fn();
  const following = followV2Agent(
    h.session,
    cursor(0),
    new AbortController().signal,
    onEvent,
    vi.fn(),
  );
  const rejected = expect(following).rejects.toBeInstanceOf(IdentityChangedError);
  await vi.advanceTimersByTimeAsync(500);
  await rejected;
  expect(events).toHaveBeenCalledOnce();
  expect(onEvent).not.toHaveBeenCalled();
  expect(sessionStorage.length).toBe(0);
});

it('rejects frames from a locally invalidated epoch before the view or cursor advances', async () => {
  const h = await opened();
  const onEvent = vi.fn();
  const acknowledge = vi.fn();
  vi.spyOn(h.session.events, 'events').mockImplementationOnce(async (_id, options) => {
    const bob = agentTransportJournal(sessionStorage, 'bob', 'agt_1');
    bob.select(true);
    await options.onEvent({
      type: 'cma.agent.event',
      data: {
        ...item(1, { type: 'turn_started', turn_sequence: 1 }),
        agent_id: 'agt_1',
        incarnation: 'a'.repeat(64),
      },
    } as Parameters<typeof options.onEvent>[0]);
    acknowledge();
  });
  await expect(
    followV2Agent(h.session, cursor(0), new AbortController().signal, onEvent, vi.fn()),
  ).rejects.toBeInstanceOf(IdentityChangedError);
  expect(onEvent).not.toHaveBeenCalled();
  expect(acknowledge).not.toHaveBeenCalled();
});

it.each(['stop', 'delete'] as const)(
  'refuses %s through a stale account view before resource mutation',
  async (action) => {
    const h = harness();
    const operation = vi.spyOn(h.client, action === 'stop' ? 'stopAgent' : 'deleteAgent');
    const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
    await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
    h.currentUser.mockResolvedValue('bob');
    await act(() => result.current[action]());
    expect(operation).not.toHaveBeenCalled();
    expect(result.current.state).toMatchObject({
      status: 'error',
      message: expect.stringContaining('sign-in changed'),
    });
    expect(result.current.state).not.toHaveProperty('conversation');
    unmount();
  },
);
