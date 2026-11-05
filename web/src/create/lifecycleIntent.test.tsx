import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { lifecycleStorageKey, useLifecycleIntent } from './lifecycleIntent';

beforeEach(() => sessionStorage.clear());
afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

const retained = (key: string, body: object) =>
  JSON.stringify({ version: 1, intent: { key, body } });

it('reads the retained request of whichever location is chosen and keeps each separate', () => {
  sessionStorage.setItem(
    lifecycleStorageKey('owner', 'wsp_1'),
    retained('workspace-key', { name: 'Research', workspace_id: 'wsp_1' }),
  );
  const client = agentClient();
  const { result, rerender } = renderHook(
    ({ workspaceId, sandboxId }: { workspaceId?: string; sandboxId?: string }) =>
      useLifecycleIntent(client, 'owner', workspaceId, sandboxId),
    { initialProps: {} as { workspaceId?: string; sandboxId?: string } },
  );
  expect(result.current.intent).toBeUndefined();
  rerender({ workspaceId: 'wsp_1' });
  expect(result.current.intent?.key).toBe('workspace-key');
  rerender({ sandboxId: 'sbx_1' });
  expect(result.current.intent).toBeUndefined();
  expect(result.current.failure).toBeUndefined();
  rerender({});
  expect(result.current.intent).toBeUndefined();
  expect(sessionStorage.getItem(lifecycleStorageKey('owner', 'wsp_1'))).toContain('workspace-key');
});

it('submits for the location chosen last and drops the previous location failure', async () => {
  const create = vi
    .fn()
    .mockRejectedValueOnce(new Error('Lost response'))
    .mockResolvedValue({ id: 'operation-two' });
  const client = agentClient({
    createAgentLifecycle: create,
    readAgentLifecycle: () => new Promise(() => undefined),
  });
  const { result, rerender } = renderHook(
    ({ workspaceId }: { workspaceId?: string }) =>
      useLifecycleIntent(client, 'owner', workspaceId, undefined),
    { initialProps: {} as { workspaceId?: string } },
  );
  await act(() => result.current.submit({ name: 'New agent' }));
  expect(result.current.failure).toEqual({ kind: 'unconfirmed', task: false });
  expect(result.current.intent?.body).toEqual({ name: 'New agent' });
  rerender({ workspaceId: 'wsp_1' });
  expect(result.current.failure).toBeUndefined();
  expect(result.current.intent).toBeUndefined();
  await act(() => result.current.submit({ name: 'New agent', workspace_id: 'wsp_1' }));
  await waitFor(() => expect(result.current.intent?.receipt?.id).toBe('operation-two'));
  expect(create.mock.calls[1]?.[0]).toEqual({ name: 'New agent', workspace_id: 'wsp_1' });
  expect(create.mock.calls[1]?.[1].idempotencyKey).not.toBe(
    create.mock.calls[0]?.[1].idempotencyKey,
  );
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toContain('"name":"New agent"');
});

it('abandons only the response of a request whose location was left', async () => {
  let signal: AbortSignal | undefined;
  const create = vi.fn(
    (_body: unknown, options: { signal?: AbortSignal }) =>
      new Promise<never>((_resolve, reject) => {
        signal = options.signal;
        options.signal?.addEventListener('abort', () => reject(new Error('aborted')));
      }),
  );
  const client = agentClient({ createAgentLifecycle: create });
  const { result, rerender } = renderHook(
    ({ sandboxId }: { sandboxId?: string }) =>
      useLifecycleIntent(client, 'owner', undefined, sandboxId),
    { initialProps: {} as { sandboxId?: string } },
  );
  act(() => void result.current.submit({ name: 'New agent' }));
  expect(result.current.pending).toBe(true);
  rerender({ sandboxId: 'sbx_1' });
  await waitFor(() => expect(signal?.aborted).toBe(true));
  expect(result.current.pending).toBe(false);
  expect(result.current.intent).toBeUndefined();
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toContain('"name":"New agent"');
});
