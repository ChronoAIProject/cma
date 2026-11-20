import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import type { AgentLifecycleOperationView } from '../api/client';
import { useLifecycleIntent } from './lifecycleIntent';
import { lifecycleRecoveryKey, lifecycleStorageKey } from './lifecycleIntentStorage';

const slot = lifecycleStorageKey('owner');
const old = {
  key: 'request-a',
  body: { name: 'Task A', initial_message: 'Original A' },
  receipt: { id: 'operation-a' },
  agentId: 'agent-a',
};
const binding = { agentId: 'agent-a', operationId: 'operation-a' };
const journal = lifecycleRecoveryKey({ owner: 'owner' }, binding);
const body = { name: 'Task B', initial_message: 'hello' };

afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

it('recovers a lost B response with its original body/key after reload while A remains recoverable', async () => {
  sessionStorage.setItem(slot, JSON.stringify({ version: 1, intent: old }));
  const create = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('Response lost'))
    .mockResolvedValue({ id: 'operation-b' });
  const client = agentClient({
    createAgentLifecycle: create,
    readAgentLifecycle: () => new Promise(() => {}),
  });
  const first = renderHook(() => useLifecycleIntent(client, 'owner'));
  expect(first.result.current.intent).toBeUndefined();
  await act(() => first.result.current.submit(body));
  const retained = sessionStorage.getItem(slot);
  expect(first.result.current.failure).toEqual({ kind: 'unconfirmed', task: true });
  first.unmount();
  const second = renderHook(() => useLifecycleIntent(client, 'owner'));
  expect(create).toHaveBeenCalledTimes(1);
  expect(sessionStorage.getItem(slot)).toBe(retained);
  // A fresh submission cannot replace an uncertain request, even through a stale handler.
  await act(() => second.result.current.submit({ ...body, initial_message: 'replacement' }));
  expect(create).toHaveBeenCalledTimes(1);
  await act(() => second.result.current.submit());
  expect(create).toHaveBeenCalledTimes(2);
  expect(create.mock.calls.map(([value]) => value)).toEqual([body, body]);
  const keys = create.mock.calls.map(([, options]) => options.idempotencyKey);
  expect(keys[0]).toBe(keys[1]);
  expect(keys[0]).not.toBe(old.key);
  expect(JSON.parse(sessionStorage.getItem(journal)!).intent).toEqual(old);
  expect(second.result.current.intent?.receipt?.id).toBe('operation-b');
});

it('allows only one of two mounted creation surfaces to admit a request', async () => {
  const create = vi.fn(() => new Promise<never>(() => {}));
  const client = agentClient({ createAgentLifecycle: create });
  const first = renderHook(() => useLifecycleIntent(client, 'owner'));
  const second = renderHook(() => useLifecycleIntent(client, 'owner'));
  act(() => {
    void first.result.current.submit(body);
    void second.result.current.submit({ ...body, initial_message: 'Other draft' });
  });
  expect(create).toHaveBeenCalledOnce();
  expect(second.result.current.blocked).toBe(true);
  expect(JSON.parse(sessionStorage.getItem(slot)!).intent.body).toEqual(body);
});

it('does not let a stale close callback abort a new owner’s request', () => {
  const create = vi.fn<NonNullable<ReturnType<typeof agentClient>['createAgentLifecycle']>>(
    () => new Promise<never>(() => {}),
  );
  const client = agentClient({ createAgentLifecycle: create });
  const view = renderHook(({ owner }) => useLifecycleIntent(client, owner), {
    initialProps: { owner: 'owner' },
  });
  const oldClose = view.result.current.abandon;
  view.rerender({ owner: 'other-owner' });
  act(() => void view.result.current.submit(body));
  act(oldClose);
  expect(create).toHaveBeenCalledOnce();
  expect(create.mock.calls[0]![1]?.signal?.aborted).toBe(false);
});

it('drops a late status or connection observation after a different pending request takes over', async () => {
  sessionStorage.setItem(
    slot,
    JSON.stringify({ version: 1, intent: { ...old, agentId: undefined } }),
  );
  let resolve!: (value: AgentLifecycleOperationView) => void;
  const client = agentClient({
    readAgentLifecycle: () =>
      new Promise((done) => {
        resolve = done;
      }),
    createAgentLifecycle: () => new Promise(() => {}),
  });
  const view = renderHook(() => useLifecycleIntent(client, 'owner'));
  await act(async () => {
    expect(view.result.current.clear()).toBe(true);
    void view.result.current.submit(body);
    resolve({
      id: 'operation-a',
      status: 'waiting_for_connection',
      phase: 'agent_startup',
      events: [],
      connection: { services: ['api-github'], target: { type: 'agent', id: 'agent-a' } },
    });
  });
  expect(view.result.current.connection).toBeUndefined();
  expect(view.result.current.operation).toBeUndefined();
  expect(view.result.current.intent?.body).toEqual(body);
});

it('fences captured clear and retain callbacks after A is archived and B is submitted', async () => {
  const unbound = { ...old, agentId: undefined };
  sessionStorage.setItem(slot, JSON.stringify({ version: 1, intent: unbound }));
  const client = agentClient({
    createAgentLifecycle: vi.fn(async () => ({ id: 'operation-b' })),
    readAgentLifecycle: async (id) => ({
      id,
      status: 'running',
      phase: 'agent_startup',
      agent_id: 'agent-a',
      events: [],
    }),
  });
  const first = renderHook(() => useLifecycleIntent(client, 'owner'));
  await waitFor(() => expect(first.result.current.operation?.agent_id).toBe('agent-a'));
  const stale = first.result.current;
  act(() => expect(first.result.current.bindAgent('agent-a')).toBe(true));
  const second = renderHook(() => useLifecycleIntent(client, 'owner'));
  await act(() => second.result.current.submit(body));
  const retained = sessionStorage.getItem(slot);
  act(() => expect(stale.clear()).toBe(false));
  act(() => expect(stale.bindAgent('agent-a')).toBe(false));
  expect(sessionStorage.getItem(slot)).toBe(retained);
  expect(JSON.parse(sessionStorage.getItem(journal)!).intent).toEqual(old);
});

it.each(['owner', 'workspace', 'sandbox', 'client', 'unmount'] as const)(
  'ignores a late creation response and captured callbacks after leaving the %s generation',
  async (change) => {
    let resolve!: (value: { id: string }) => void;
    const create = vi.fn(
      () =>
        new Promise<{ id: string }>((done) => {
          resolve = done;
        }),
    );
    const client = agentClient({ createAgentLifecycle: create });
    const props = {
      client,
      owner: 'owner',
      workspace: undefined as string | undefined,
      sandbox: undefined as string | undefined,
    };
    const view = renderHook(
      ({ client, owner, workspace, sandbox }) =>
        useLifecycleIntent(client, owner, workspace, sandbox),
      { initialProps: props },
    );
    let pending!: Promise<void>;
    act(() => {
      pending = view.result.current.submit(body);
    });
    const stale = view.result.current;
    const retained = sessionStorage.getItem(slot);
    if (change === 'unmount') view.unmount();
    else
      view.rerender({
        ...props,
        ...(change === 'owner'
          ? { owner: 'other' }
          : change === 'workspace'
            ? { workspace: 'other' }
            : change === 'sandbox'
              ? { sandbox: 'other' }
              : { client: agentClient() }),
      });
    await act(async () => {
      resolve({ id: 'late-operation' });
      await pending;
    });
    act(() => expect(stale.clear()).toBe(false));
    await act(() => stale.submit());
    expect(create).toHaveBeenCalledOnce();
    expect(sessionStorage.getItem(slot)).toBe(retained);
  },
);
