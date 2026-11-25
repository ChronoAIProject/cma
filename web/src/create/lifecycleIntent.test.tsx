import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { lifecycleStorageKey, useLifecycleIntent } from './lifecycleIntent';
import type { AgentLifecycleOperationView } from '../api/client';
import { openTaskDraftStore } from './taskDraftStorage';

beforeEach(() => sessionStorage.clear());
afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

const retained = (key: string, body: object) =>
  JSON.stringify({ version: 1, intent: { key, body } });

const connectionWait: AgentLifecycleOperationView = {
  id: 'original-operation',
  status: 'waiting_for_connection',
  phase: 'sandbox_ready',
  connection: {
    services: ['api-github'],
    target: { type: 'sandbox_profile', profile: { id: 'profile-owned', revision: 1 } },
  },
  events: [],
};

it('preserves a newer task draft and fences a delayed continuation after discard', async () => {
  const slot = lifecycleStorageKey('owner');
  sessionStorage.setItem(
    slot,
    JSON.stringify({
      version: 1,
      intent: {
        key: 'original-key',
        body: { name: 'Original', initial_message: 'Old task' },
        draftRevision: 'old-draft',
        receipt: { id: connectionWait.id },
      },
    }),
  );
  const store = openTaskDraftStore('owner');
  const newer = { ...store.read().draft, revision: 'new-draft', text: 'Independent draft' };
  store.save(newer);
  const create = vi.fn(async () => ({ id: 'independent-operation' }));
  const client = agentClient({
    createAgentLifecycle: create,
    readAgentLifecycle: vi
      .fn()
      .mockResolvedValueOnce(connectionWait)
      .mockImplementation(() => new Promise(() => undefined)),
  });
  const { result } = renderHook(() => useLifecycleIntent(client, 'owner'));
  await waitFor(() => expect(result.current.canDiscardWaiting).toBe(true));
  const continueOriginal = result.current.submit;
  act(() => expect(result.current.discardWaiting()).toBe(true));
  expect(sessionStorage.getItem(slot)).toBeNull();
  expect(store.read().draft).toEqual(newer);
  await act(() => result.current.submit({ name: 'Next task' }));
  await act(() => continueOriginal());
  expect(create).toHaveBeenCalledOnce();
});

it('discards a revisionless receipt while preserving a separately saved identical draft on reload', async () => {
  const slot = lifecycleStorageKey('owner');
  const repository = { url: 'https://github.com/example/project', ref: 'main' };
  sessionStorage.setItem(
    slot,
    JSON.stringify({
      version: 1,
      intent: {
        key: 'original-key',
        body: { name: 'Original', initial_message: 'Inspect the source', repository },
        receipt: { id: connectionWait.id },
      },
    }),
  );
  const store = openTaskDraftStore('owner');
  const newer = {
    ...store.read().draft,
    revision: 'new-draft',
    text: 'Inspect the source',
    repositoryUrl: repository.url,
    repositoryRef: repository.ref,
  };
  store.save(newer);
  const create = vi.fn(async () => ({ id: 'fresh-operation' }));
  const client = agentClient({
    createAgentLifecycle: create,
    readAgentLifecycle: async (id) => ({ ...connectionWait, id }),
  });
  const first = renderHook(() => useLifecycleIntent(client, 'owner'));
  await waitFor(() => expect(first.result.current.canDiscardWaiting).toBe(true));
  act(() => expect(first.result.current.discardWaiting()).toBe(true));
  expect(sessionStorage.getItem(slot)).toBeNull();
  first.unmount();
  const reloaded = renderHook(() => useLifecycleIntent(client, 'owner'));
  expect(reloaded.result.current.intent).toBeUndefined();
  expect(openTaskDraftStore('owner').read().draft).toEqual(newer);
  await act(() => reloaded.result.current.submit({ name: 'Independent task' }));
  expect(create).toHaveBeenCalledOnce();
  expect(reloaded.result.current.intent?.key).not.toBe('original-key');
});

it.each(['queued', 'running', 'unconfirmed', 'admitted'] as const)(
  'preserves %s creation recovery on discard',
  async (state) => {
    const slot = lifecycleStorageKey('owner');
    const saved = JSON.stringify({
      version: 1,
      intent: {
        key: 'original-key',
        body: { name: 'Original' },
        ...(state === 'unconfirmed' ? {} : { receipt: { id: connectionWait.id } }),
      },
    });
    sessionStorage.setItem(slot, saved);
    const client = agentClient({
      readAgentLifecycle: async () =>
        state === 'admitted'
          ? { ...connectionWait, agent_id: 'agt_1' }
          : { ...connectionWait, status: state },
    });
    const { result } = renderHook(() => useLifecycleIntent(client, 'owner'));
    if (state !== 'unconfirmed')
      await waitFor(() => expect(result.current.operation).toBeDefined());
    expect(result.current.canDiscardWaiting).toBe(false);
    act(() => expect(result.current.discardWaiting()).toBe(false));
    expect(sessionStorage.getItem(slot)).toBe(saved);
  },
);

it('preserves a waiting receipt when discard races an uncertain replay', async () => {
  const slot = lifecycleStorageKey('owner');
  const saved = JSON.stringify({
    version: 1,
    intent: {
      key: 'original-key',
      body: { name: 'Original' },
      receipt: { id: connectionWait.id },
    },
  });
  sessionStorage.setItem(slot, saved);
  const client = agentClient({
    createAgentLifecycle: vi.fn().mockRejectedValue(new TypeError('Response lost')),
    readAgentLifecycle: async () => connectionWait,
  });
  const { result } = renderHook(() => useLifecycleIntent(client, 'owner'));
  await waitFor(() => expect(result.current.canDiscardWaiting).toBe(true));
  const discard = result.current.discardWaiting;
  await act(async () => {
    const retry = result.current.submit();
    expect(discard()).toBe(false);
    await retry;
    expect(discard()).toBe(false);
  });
  expect(sessionStorage.getItem(slot)).toBe(saved);
  expect(result.current.canDiscardWaiting).toBe(false);
});

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

it.each([undefined, 'automatic', 'named'] as const)(
  'restores the exact %s title request only for its original owner',
  (title_mode) => {
    const body = {
      name: 'New task',
      initial_message: 'Original task',
      ...(title_mode ? { title_mode } : {}),
    };
    sessionStorage.setItem(lifecycleStorageKey('owner'), retained('original-key', body));
    const client = agentClient();
    const { result, rerender } = renderHook(({ owner }) => useLifecycleIntent(client, owner), {
      initialProps: { owner: 'owner' },
    });
    expect(result.current.intent).toEqual({ key: 'original-key', body });
    rerender({ owner: 'other-owner' });
    expect(result.current.intent).toBeUndefined();
    rerender({ owner: 'owner' });
    expect(result.current.intent).toEqual({ key: 'original-key', body });
  },
);

it('retains an unreadable title mode without issuing a request', () => {
  const raw = retained('original-key', { name: 'New task', title_mode: 'inferred' });
  sessionStorage.setItem(lifecycleStorageKey('owner'), raw);
  const create = vi.fn();
  const { result } = renderHook(() =>
    useLifecycleIntent(agentClient({ createAgentLifecycle: create }), 'owner'),
  );
  expect(result.current.intent).toBeUndefined();
  expect(result.current.blocked).toBe(true);
  expect(sessionStorage.getItem(lifecycleStorageKey('owner'))).toBe(raw);
  expect(create).not.toHaveBeenCalled();
});
