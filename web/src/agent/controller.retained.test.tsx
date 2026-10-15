import { act, renderHook, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, AGENT_USAGE, agentClient } from '../test/agent';
import { useAgentController } from './controller';

function fixture() {
  return agentClient({
    readAgent: vi.fn(async (id) => ({ ...AGENT_DETAIL, id })),
    readAgentTranscript: vi.fn(async (agentId) => ({ ...AGENT_TRANSCRIPT, agentId })),
    readAgentUsage: vi.fn(async (agentId) => ({ ...AGENT_USAGE, agentId })),
    readAgentEvents: vi.fn(async () => ({ items: [] })),
    replayAgent: vi.fn(
      async (_id, { signal }) =>
        new Promise<void>((resolve) => {
          signal?.addEventListener('abort', () => resolve(), { once: true });
        }),
    ),
    runAgent: vi.fn(),
  });
}

it('restores a visited Agent immediately and resumes replay without downloading its history again', async () => {
  const client = fixture();
  const view = renderHook(
    ({ id, suspended }) =>
      useAgentController(client, id, {
        viewerId: 'alice',
        suspended,
      }),
    { initialProps: { id: 'agt_1', suspended: false } },
  );
  await waitFor(() => expect(view.result.current.state.status).toBe('ready'));
  view.rerender({ id: 'agt_2', suspended: false });
  await waitFor(() =>
    expect(view.result.current.state).toMatchObject({ status: 'ready', detail: { id: 'agt_2' } }),
  );
  view.rerender({ id: 'agt_1', suspended: true });
  expect(view.result.current.state).toMatchObject({
    status: 'ready',
    detail: { id: 'agt_1' },
    conversation: {
      messages: expect.arrayContaining([
        expect.objectContaining({ content: 'The repository is ready.' }),
      ]),
    },
  });
  await act(() => view.result.current.send('Not authorized yet'));
  expect(client.runAgent).not.toHaveBeenCalled();
  view.rerender({ id: 'agt_1', suspended: false });
  await waitFor(() => expect(client.readAgent).toHaveBeenCalledTimes(3));
  expect(client.readAgentTranscript).toHaveBeenCalledTimes(2);
  expect(client.readAgentEvents).toHaveBeenCalledTimes(2);
  expect(client.replayAgent).toHaveBeenLastCalledWith(
    'agt_1',
    expect.objectContaining({ after: 4 }),
  );
  view.unmount();
});

it('never shows the previous account view while the next account is still being checked', async () => {
  const client = fixture();
  const view = renderHook(
    ({ owner, suspended }) =>
      useAgentController(client, 'agt_1', {
        viewerId: owner,
        suspended,
      }),
    { initialProps: { owner: 'alice', suspended: false } },
  );
  await waitFor(() => expect(view.result.current.state.status).toBe('ready'));
  view.rerender({ owner: 'bob', suspended: true });
  expect(view.result.current.state.status).toBe('loading');
  expect(view.result.current.inputAvailability.allowed).toBe(false);
  view.unmount();
});

it('discards a retained view when its fresh metadata read denies access', async () => {
  const client = fixture();
  const view = renderHook(({ id }) => useAgentController(client, id, { viewerId: 'alice' }), {
    initialProps: { id: 'agt_1' },
  });
  await waitFor(() => expect(view.result.current.state.status).toBe('ready'));
  view.rerender({ id: 'agt_2' });
  await waitFor(() =>
    expect(view.result.current.state).toMatchObject({ status: 'ready', detail: { id: 'agt_2' } }),
  );
  vi.mocked(client.readAgent).mockRejectedValueOnce(new ApiError(403, 'Access removed'));
  view.rerender({ id: 'agt_1' });
  await waitFor(() =>
    expect(view.result.current.state).toEqual({ status: 'error', message: 'Access removed' }),
  );
  expect(client.runAgent).not.toHaveBeenCalled();
  view.unmount();
});
