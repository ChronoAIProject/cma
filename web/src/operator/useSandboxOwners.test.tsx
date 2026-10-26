import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { operatorClient, sandboxRow } from './testFixtures';
import { useSandboxOwners } from './useSandboxOwners';

afterEach(() => vi.useRealTimers());

it('rejects repeated pagination instead of presenting an incomplete owner list', async () => {
  const client = operatorClient();
  client.admin.listSandboxes.mockResolvedValue({ items: [sandboxRow], next: 'repeated' });
  const { result } = renderHook(() => useSandboxOwners(client));
  await waitFor(() => expect(result.current.state.status).toBe('error'));
  expect(client.admin.listSandboxes).toHaveBeenCalledTimes(2);
});

it('aborts owner loading on unmount and does not follow a late page', async () => {
  const client = operatorClient();
  let resolve!: (value: { items: (typeof sandboxRow)[]; next: string }) => void;
  client.admin.listSandboxes.mockReturnValue(
    new Promise((done) => {
      resolve = done;
    }),
  );
  const view = renderHook(() => useSandboxOwners(client));
  const request = client.admin.listSandboxes.mock.calls[0]?.[0];
  view.unmount();
  expect(request?.signal?.aborted).toBe(true);
  await act(async () => resolve({ items: [sandboxRow], next: 'next' }));
  expect(client.admin.listSandboxes).toHaveBeenCalledOnce();
});

it('bounds a hung inventory request and permits an explicit retry', async () => {
  vi.useFakeTimers();
  const client = operatorClient();
  client.admin.listSandboxes.mockReturnValueOnce(new Promise(() => undefined));
  const { result } = renderHook(() => useSandboxOwners(client));
  await act(async () => vi.advanceTimersByTimeAsync(15_000));
  expect(client.admin.listSandboxes.mock.calls[0]?.[0]?.signal?.aborted).toBe(true);
  expect(result.current.state.status).toBe('error');
  await act(async () => result.current.retry());
  expect(result.current.state).toEqual({
    status: 'ready',
    options: [{ value: 'owner-1', label: 'Alex Chen' }],
  });
});

it('discards names immediately when the authenticated client changes', async () => {
  const oldClient = operatorClient();
  const newClient = operatorClient();
  newClient.admin.listSandboxes.mockReturnValue(new Promise(() => undefined));
  const view = renderHook(({ client }) => useSandboxOwners(client), {
    initialProps: { client: oldClient },
  });
  await waitFor(() => expect(view.result.current.state.status).toBe('ready'));
  view.rerender({ client: newClient });
  expect(view.result.current.state).toEqual({ status: 'loading' });
  expect(oldClient.admin.listSandboxes.mock.calls[0]?.[0]?.signal?.aborted).toBe(true);
});
