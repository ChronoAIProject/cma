import { waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import type { Dispatch, SetStateAction } from 'react';
import { AGENT_DETAIL, AGENT_TRANSCRIPT } from '../test/agent';
import { workspaceFixture } from '../test/hierarchy';
import type { AgentResourceState } from './controllerTypes';
import { conversationFromTranscript } from './conversation';
import { readPreparation } from './preparationRead';
import { harness } from './transport/controller.fixture';

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

beforeEach(() => sessionStorage.clear());

async function setup() {
  const h = harness();
  const v2 = await h.client.agentTransport!.open('agt_1');
  if (!v2) throw new Error('Expected v2 fixture');
  const firstMessage = {
    state: 'execution_observed' as const,
    queuedAt: AGENT_DETAIL.createdAt,
  };
  const detail = { ...AGENT_DETAIL, firstMessage };
  const conversation = conversationFromTranscript(AGENT_TRANSCRIPT);
  const state: { current: AgentResourceState } = {
    current: {
      status: 'ready',
      detail: { ...detail, status: 'starting' },
      workspace: workspaceFixture(),
      conversation,
      v2,
    },
  };
  const publish = vi.fn<Dispatch<SetStateAction<AgentResourceState>>>((next) => {
    state.current = typeof next === 'function' ? next(state.current) : next;
  });
  const abort = new AbortController();
  const start = () =>
    readPreparation(h.client, 'agt_1', abort.signal, state, publish).then(
      (value) => ({ value, error: undefined }),
      (error: unknown) => ({ value: undefined, error }),
    );
  h.currentUser.mockClear();
  return { ...h, state, detail, conversation, publish, abort, start };
}

it.each(['alice', 'bob'])(
  'uses one fresh account read for readiness publication and snapshot entry, then rechecks before returning history (%s)',
  async (finalOwner) => {
    const h = await setup();
    const transition = deferred<string | undefined>();
    const final = deferred<string | undefined>();
    const history = deferred<typeof AGENT_TRANSCRIPT>();
    h.currentUser
      .mockResolvedValueOnce('alice')
      .mockReturnValueOnce(transition.promise)
      .mockReturnValueOnce(final.promise);
    const readAgent = vi.spyOn(h.client, 'readAgent').mockResolvedValue(h.detail);
    vi.mocked(h.client.readAgentTranscript).mockReturnValue(history.promise);
    const result = h.start();
    await waitFor(() => expect(h.currentUser).toHaveBeenCalledTimes(2));
    expect(readAgent).toHaveBeenCalledOnce();
    expect(h.publish).not.toHaveBeenCalled();
    expect(h.snapshot).not.toHaveBeenCalled();

    transition.resolve('alice');
    await waitFor(() => expect(h.snapshot).toHaveBeenCalledOnce());
    expect(readAgent).toHaveBeenCalledTimes(2);
    expect(h.currentUser).toHaveBeenCalledTimes(2);
    expect(h.publish).toHaveBeenCalledOnce();
    expect(h.state.current).toMatchObject({ status: 'ready', detail: h.detail });
    if (h.state.current.status !== 'ready') throw new Error('Expected ready state');
    expect(h.state.current.conversation).toBe(h.conversation);

    history.resolve({ agentId: 'agt_1', throughSeq: 0, turns: [] });
    await waitFor(() => expect(h.currentUser).toHaveBeenCalledTimes(3));
    final.resolve(finalOwner);
    const completed = await result;
    if (finalOwner === 'alice') {
      expect(completed.error).toBeUndefined();
      expect(completed.value?.detail).toEqual(h.detail);
      expect(completed.value?.v2).not.toBe(h.state.current.v2);
      expect(completed.value?.v2?.journal).toBe(h.state.current.v2?.journal);
    } else {
      expect(completed.value).toBeUndefined();
      expect(completed.error).toBeInstanceOf(Error);
    }
  },
);

it('refuses readiness publication and snapshot reads if the account changes during the transition', async () => {
  const h = await setup();
  h.currentUser.mockResolvedValueOnce('alice').mockResolvedValueOnce('bob');
  const readAgent = vi.spyOn(h.client, 'readAgent').mockResolvedValue(h.detail);
  const result = await h.start();
  expect(result.value).toBeUndefined();
  expect(result.error).toBeInstanceOf(Error);
  expect(readAgent).toHaveBeenCalledOnce();
  expect(h.publish).not.toHaveBeenCalled();
  expect(h.snapshot).not.toHaveBeenCalled();
});
