import { afterEach, expect, it, vi } from 'vitest';
import {
  lifecycleRecoveryKey,
  lifecycleScopeForAgent,
  lifecycleStorageKey,
  loadLifecycleIntent,
  retainLifecycleIntent,
  type Intent,
} from './lifecycleIntentStorage';

const scope = { owner: 'owner' };
const binding = { agentId: 'agent-a', operationId: 'operation-a' };
const pending = lifecycleStorageKey('owner');
const recovery = lifecycleRecoveryKey(scope, binding);
const original: Intent = {
  key: 'request-a',
  body: {
    name: 'Original task',
    initial_message: 'First task',
    repository: { url: 'https://github.com/example/project', ref: 'main' },
    machine_included: true,
  },
  receipt: { id: binding.operationId, status: 'running' },
  agentId: binding.agentId,
};
const next: Intent = { key: 'request-b', body: { name: 'New task', initial_message: 'hello' } };
const encode = (intent: Intent) => JSON.stringify({ version: 1, intent });

afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

it.each([{}, { workspaceId: 'workspace-a' }, { sandboxId: 'sandbox-a' }])(
  'moves a legacy bound request intact to its own recovery journal (%j)',
  (context) => {
    const location = { ...scope, ...context };
    const intent = {
      ...original,
      body: { ...original.body, workspace_id: context.workspaceId, sandbox_id: context.sandboxId },
    };
    const slot = lifecycleStorageKey('owner', context.workspaceId, context.sandboxId);
    const journal = lifecycleRecoveryKey(location, binding);
    sessionStorage.setItem(slot, encode(intent));
    sessionStorage.setItem('unrelated-preference', 'keep');
    expect(loadLifecycleIntent(location)).toEqual({ intent: undefined });
    expect(sessionStorage.getItem(slot)).toBeNull();
    expect(sessionStorage.getItem(journal)).toBe(encode(intent));
    expect(loadLifecycleIntent(location, binding)).toEqual({ intent });
    expect(lifecycleScopeForAgent('owner', binding.agentId)).toEqual({
      workspaceId: context.workspaceId,
      sandboxId: context.sandboxId,
      operationId: binding.operationId,
    });
    expect(sessionStorage.getItem('unrelated-preference')).toBe('keep');
  },
);

it('keeps an unbound uncertain request in its original admission slot', () => {
  const intent = { ...original, agentId: undefined };
  sessionStorage.setItem(pending, encode(intent));
  expect(loadLifecycleIntent(scope)).toEqual({ intent });
  expect(sessionStorage.getItem(pending)).toBe(encode(intent));
  expect(sessionStorage.getItem(recovery)).toBeNull();
});

it.each([undefined, 'named', 'automatic'] as const)(
  'preserves %s title intent and draft ownership through recovery journal migration',
  (title_mode) => {
    const intent = {
      ...original,
      draftRevision: 'draft-a',
      body: { ...original.body, ...(title_mode ? { title_mode } : {}) },
    };
    sessionStorage.setItem(pending, encode(intent));
    expect(loadLifecycleIntent(scope)).toEqual({ intent: undefined });
    expect(sessionStorage.getItem(recovery)).toBe(encode(intent));
    retainLifecycleIntent(scope, undefined, next);
    expect(loadLifecycleIntent(scope, binding)).toEqual({ intent });
    retainLifecycleIntent(scope, intent);
    expect(loadLifecycleIntent(scope)).toEqual({ intent: next });
  },
);

it.each(['unsupported', null, false, 1])(
  'refuses an invalid title mode %j without migrating or discarding the request',
  (title_mode) => {
    const raw = JSON.stringify({
      version: 1,
      intent: { ...original, body: { ...original.body, title_mode } },
    });
    sessionStorage.setItem(pending, raw);
    expect(loadLifecycleIntent(scope).error).toBeDefined();
    expect(sessionStorage.getItem(pending)).toBe(raw);
    expect(sessionStorage.getItem(recovery)).toBeNull();
    expect(lifecycleScopeForAgent(scope.owner, binding.agentId)).toBeUndefined();
    sessionStorage.removeItem(pending);
    sessionStorage.setItem(recovery, raw);
    expect(loadLifecycleIntent(scope, binding).error).toBeDefined();
    expect(lifecycleScopeForAgent(scope.owner, binding.agentId)).toBeUndefined();
  },
);

it.each(['getItem', 'setItem', 'removeItem'] as const)(
  'fails closed and retains the legacy identity when storage %s throws',
  (method) => {
    sessionStorage.setItem(pending, encode(original));
    const spy = vi.spyOn(Storage.prototype, method).mockImplementation(() => {
      throw new DOMException('Storage unavailable');
    });
    expect(loadLifecycleIntent(scope).error).toMatch(/Copy your task/);
    spy.mockRestore();
    expect(sessionStorage.getItem(pending)).toBe(encode(original));
    if (method === 'removeItem') expect(sessionStorage.getItem(recovery)).toBe(encode(original));
    expect(loadLifecycleIntent(scope)).toEqual({ intent: undefined });
    expect(sessionStorage.getItem(recovery)).toBe(encode(original));
  },
);

it.each(['setItem', 'removeItem'] as const)(
  'verifies that storage %s actually took effect',
  (method) => {
    sessionStorage.setItem(pending, encode(original));
    const spy = vi.spyOn(Storage.prototype, method).mockImplementation(() => undefined);
    expect(loadLifecycleIntent(scope).error).toBeDefined();
    spy.mockRestore();
    expect(sessionStorage.getItem(pending)).toBe(encode(original));
  },
);

it.each([
  'broken json',
  JSON.stringify({ version: 2, intent: original }),
  encode({ ...original, key: '' }),
  encode({ ...original, body: { ...original.body, workspace_id: 'another-workspace' } }),
  encode({ ...original, receipt: undefined }),
])('refuses malformed or mismatched legacy data without removing it: %s', (raw) => {
  sessionStorage.setItem(pending, raw);
  expect(loadLifecycleIntent(scope).error).toBeDefined();
  expect(sessionStorage.getItem(pending)).toBe(raw);
  expect(sessionStorage.getItem(recovery)).toBeNull();
});

it('does not overwrite an existing recovery identity during migration', () => {
  sessionStorage.setItem(pending, encode(original));
  const conflict = encode({ ...original, key: 'different-request' });
  sessionStorage.setItem(recovery, conflict);
  expect(loadLifecycleIntent(scope).error).toBeDefined();
  expect(sessionStorage.getItem(pending)).toBe(encode(original));
  expect(sessionStorage.getItem(recovery)).toBe(conflict);
});

it('rejects old clear, retain and bind calls after a new request owns the pending slot', () => {
  const unbound = { ...original, agentId: undefined };
  sessionStorage.setItem(pending, encode(unbound));
  retainLifecycleIntent(scope, unbound, original);
  retainLifecycleIntent(scope, undefined, next);
  for (const update of [undefined, unbound, original]) {
    expect(() => retainLifecycleIntent(scope, unbound, update)).toThrow(/saved request changed/);
    expect(sessionStorage.getItem(pending)).toBe(encode(next));
  }
  retainLifecycleIntent(scope, original);
  expect(sessionStorage.getItem(recovery)).toBeNull();
  expect(sessionStorage.getItem(pending)).toBe(encode(next));
  expect(() => retainLifecycleIntent(scope, original, original)).toThrow();
});

it('compares the complete body and receipt, even when a request key is reused', () => {
  sessionStorage.setItem(pending, encode(next));
  expect(() => retainLifecycleIntent(scope, { ...next, body: original.body })).toThrow();
  const accepted = { ...next, receipt: { id: 'operation-b' } };
  retainLifecycleIntent(scope, next, accepted);
  expect(() => retainLifecycleIntent(scope, next)).toThrow();
  expect(() =>
    retainLifecycleIntent(scope, accepted, { ...accepted, receipt: { id: 'operation-c' } }),
  ).toThrow();
  expect(sessionStorage.getItem(pending)).toBe(encode(accepted));
});

it('keeps Agent recovery readable even when the next pending request is corrupt', () => {
  sessionStorage.setItem(recovery, encode(original));
  sessionStorage.setItem(pending, 'corrupt new request');
  expect(loadLifecycleIntent(scope, binding)).toEqual({ intent: original });
  retainLifecycleIntent(scope, original);
  expect(sessionStorage.getItem(pending)).toBe('corrupt new request');
});

it('isolates origin, owner, Agent, operation and context on journal reads', () => {
  sessionStorage.setItem(recovery, encode(original));
  expect(loadLifecycleIntent({ owner: 'other' }, binding).intent).toBeUndefined();
  expect(loadLifecycleIntent({ ...scope, workspaceId: 'other' }, binding).intent).toBeUndefined();
  expect(loadLifecycleIntent(scope, { ...binding, agentId: 'other' }).intent).toBeUndefined();
  expect(loadLifecycleIntent(scope, { ...binding, operationId: 'other' }).intent).toBeUndefined();
  expect(lifecycleScopeForAgent('other', binding.agentId)).toBeUndefined();
  sessionStorage.removeItem(recovery);
  sessionStorage.setItem(
    recovery.replace(location.origin, 'https://example.invalid'),
    encode(original),
  );
  expect(lifecycleScopeForAgent('owner', binding.agentId)).toBeUndefined();
  expect(loadLifecycleIntent(scope, binding).intent).toBeUndefined();
});
