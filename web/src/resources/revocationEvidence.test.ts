import { beforeEach, expect, it, vi } from 'vitest';
import type { ResourceRevocationStatus } from '@cma/client';
import { retainTaskViewer } from '../create/taskIdentity';
import { actor, fixture, operation, original, target } from './ShareChangeRecovery.fixture';
import { readShareChanges, saveShareChange } from './retainedShareChange';
import { readRevocationEvidence, retainRevocationEvidence } from './revocationEvidence';
import { observePendingRevocation, observeRevocationEffects } from './revocationObservation';

const scope = { owner: 'owner', kind: 'sandbox', id: 'sandbox' } as const;
const intent = {
  key: 'original-request',
  shareId: original.id,
  revision: 1,
  organization: 'org',
  mode: 'view',
  revoked: false,
  operation,
} as const;
const key = 'cma:resource-revocation:observed:["owner","sandbox","sandbox"]';
function status(op = operation, revision = 1, unresolved = 1): ResourceRevocationStatus {
  return {
    operation_id: op,
    selector: original.selector,
    share_id: original.id,
    expected_revision: revision,
    state: 'effective',
    participants: 1,
    stopped: 1,
    unresolved_effects: unresolved,
    deadline_unix_ms: 11000,
    deadline_exceeded: false,
    receipt: {
      operation_id: op,
      actor,
      at: original.created_at,
      policy_revision: revision + 1,
      share: { ...original, mode: 'view', revision: revision + 1 },
    },
  };
}
beforeEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
  retainTaskViewer();
});

it('retains independent observations for successive changes to one share and clears only the original zero', async () => {
  const second = 'b'.repeat(64);
  retainRevocationEvidence(scope, status());
  retainRevocationEvidence(scope, status(second, 2));
  const records = readRevocationEvidence(scope);
  expect(records).toHaveLength(2);
  expect(records[0]).not.toHaveProperty('key');
  const test = fixture();
  const read = vi
    .spyOn(test.client.resourceRevocations!, 'status')
    .mockResolvedValue(status(operation, 1, 0));
  await observeRevocationEffects(test.client, scope, records[0]!, new AbortController().signal);
  expect(read).toHaveBeenCalledWith(operation, 'sandbox', 'sandbox', expect.any(Object));
  expect(readRevocationEvidence(scope)).toEqual([records[1]]);
  expect(test.writes).toEqual([]);
});

it.each([
  'unavailable',
  'pending',
  'operation',
  'resource',
  'revision',
  'mode',
  'organization',
] as const)('keeps evidence when a later %s result cannot prove resolution', async (fault) => {
  retainRevocationEvidence(scope, status());
  const records = readRevocationEvidence(scope);
  const test = fixture();
  const zero = status(operation, 1, 0);
  const reply =
    fault === 'pending'
      ? { ...zero, state: 'pending' as const, receipt: null }
      : fault === 'operation'
        ? status('b'.repeat(64), 1, 0)
        : fault === 'resource'
          ? { ...zero, selector: { kind: 'sandbox' as const, sandbox_id: 'other' } }
          : fault === 'revision'
            ? status(operation, 2, 0)
            : {
                ...zero,
                receipt: {
                  ...zero.receipt!,
                  share: {
                    ...zero.receipt!.share,
                    ...(fault === 'mode' ? { mode: 'org_roles' as const } : {}),
                    ...(fault === 'organization' ? { target_org_id: 'other' } : {}),
                  },
                },
              };
  const read = vi.spyOn(test.client.resourceRevocations!, 'status');
  if (fault === 'unavailable') read.mockRejectedValue(new Error('unavailable'));
  else read.mockResolvedValue(reply);
  await expect(
    observeRevocationEffects(test.client, scope, records[0]!, new AbortController().signal),
  ).rejects.toThrow();
  expect(readRevocationEvidence(scope)).toEqual(records);
  expect(test.writes).toEqual([]);
});

it('refuses capacity overflow without eviction and permits an identical completion at capacity', () => {
  for (let n = 0; n < 64; n++)
    retainRevocationEvidence(scope, status(n.toString(16).padStart(64, '0')));
  const before = sessionStorage.getItem(key);
  expect(() => retainRevocationEvidence(scope, status())).toThrow(/Too many/);
  expect(sessionStorage.getItem(key)).toBe(before);
  retainRevocationEvidence(scope, status('0'.repeat(64)));
  expect(sessionStorage.getItem(key)).toBe(before);
});

it.each([
  '{',
  ' '.repeat(65537),
  JSON.stringify({ version: 2, records: [] }),
  JSON.stringify({ version: 1, records: [{ operation }] }),
])('refuses unreadable retained evidence without replacing it', (raw) => {
  sessionStorage.setItem(key, raw);
  expect(() => readRevocationEvidence(scope)).toThrow();
  expect(() => retainRevocationEvidence(scope, status())).toThrow();
  expect(sessionStorage.getItem(key)).toBe(raw);
});

it('repairs a known operation before a refused archive write and retains pending recovery', async () => {
  saveShareChange('owner', target, { ...intent, operation: undefined });
  const test = fixture();
  vi.spyOn(test.client.resourceRevocations!, 'status').mockResolvedValue(status());
  const set = Storage.prototype.setItem;
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (this: Storage, name, value) {
    if (name === key) throw new Error('quota');
    set.call(this, name, value);
  });
  const result = await observePendingRevocation(
    test.client,
    scope,
    intent,
    intent,
    new AbortController().signal,
  );
  expect(result.status.state).toBe('effective');
  expect(result.retained).toBe(false);
  expect(readShareChanges('owner', target)).toEqual([intent]);
  expect(readRevocationEvidence(scope)).toEqual([]);
  expect(test.writes).toEqual([]);
});

it('keeps both recovery records when pending deletion fails and completes idempotently afterward', async () => {
  saveShareChange('owner', target, intent);
  const test = fixture();
  vi.spyOn(test.client.resourceRevocations!, 'status').mockResolvedValue(status());
  const remove = vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(() => {
    throw new Error('storage');
  });
  const observe = () =>
    observePendingRevocation(test.client, scope, intent, intent, new AbortController().signal);
  expect((await observe()).retained).toBe(false);
  expect(readRevocationEvidence(scope)).toHaveLength(1);
  expect(readShareChanges('owner', target)).toEqual([intent]);
  remove.mockRestore();
  expect((await observe()).retained).toBe(true);
  expect(readShareChanges('owner', target)).toEqual([]);
  expect(readRevocationEvidence(scope)).toHaveLength(1);
  expect(test.writes).toEqual([]);
});

it('does not retire a newer pending command with an older completion', async () => {
  const newer = { ...intent, key: 'newer-request', revision: 2, operation: 'b'.repeat(64) };
  saveShareChange('owner', target, newer);
  const test = fixture();
  vi.spyOn(test.client.resourceRevocations!, 'status').mockResolvedValue(status());
  const result = await observePendingRevocation(
    test.client,
    scope,
    intent,
    intent,
    new AbortController().signal,
  );
  expect(result.retained).toBe(false);
  expect(readShareChanges('owner', target)).toEqual([newer]);
  expect(readRevocationEvidence(scope)).toEqual([]);
});

it('does not recreate positive evidence from a completion older than a successful zero read', async () => {
  saveShareChange('owner', target, intent);
  retainRevocationEvidence(scope, status());
  const evidence = readRevocationEvidence(scope)[0]!;
  const test = fixture();
  let finish!: (value: ResourceRevocationStatus) => void;
  vi.spyOn(test.client.resourceRevocations!, 'status')
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          finish = resolve;
        }),
    )
    .mockResolvedValueOnce(status(operation, 1, 0));
  const old = observePendingRevocation(
    test.client,
    scope,
    intent,
    intent,
    new AbortController().signal,
  );
  await observeRevocationEffects(test.client, scope, evidence, new AbortController().signal);
  finish(status());
  await expect(old).rejects.toThrow(/superseded/);
  expect(readRevocationEvidence(scope)).toEqual([]);
  expect(readShareChanges('owner', target)).toEqual([intent]);
});

it('ignores a late zero after cancellation even when the transport ignores abort', async () => {
  retainRevocationEvidence(scope, status());
  const evidence = readRevocationEvidence(scope)[0]!;
  const test = fixture();
  let finish!: (value: ResourceRevocationStatus) => void;
  vi.spyOn(test.client.resourceRevocations!, 'status').mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const request = new AbortController();
  const read = observeRevocationEffects(test.client, scope, evidence, request.signal);
  request.abort();
  await expect(read).rejects.toThrow();
  finish(status(operation, 1, 0));
  await Promise.resolve();
  expect(readRevocationEvidence(scope)).toEqual([evidence]);
});

it('isolates resource evidence and purges it on account switch and sign-out', () => {
  retainTaskViewer('owner');
  retainRevocationEvidence(scope, status());
  expect(readRevocationEvidence({ ...scope, id: 'other' })).toEqual([]);
  expect(() => readRevocationEvidence({ ...scope, owner: 'other' })).toThrow();
  retainTaskViewer('other');
  expect(sessionStorage.getItem(key)).toBeNull();
  retainTaskViewer('owner');
  retainRevocationEvidence(scope, status());
  retainTaskViewer();
  expect(sessionStorage.getItem(key)).toBeNull();
});

it('bounds concurrent history reads and releases observation capacity on abort', async () => {
  for (let n = 0; n < 5; n++)
    retainRevocationEvidence(scope, status(n.toString(16).padStart(64, '0')));
  const records = readRevocationEvidence(scope);
  const test = fixture();
  const read = vi
    .spyOn(test.client.resourceRevocations!, 'status')
    .mockImplementation(() => new Promise(() => {}));
  const requests = records.slice(0, 4).map(() => new AbortController());
  const pending = requests.map((request, index) =>
    observeRevocationEffects(test.client, scope, records[index]!, request.signal),
  );
  await expect(
    observeRevocationEffects(test.client, scope, records[4]!, new AbortController().signal),
  ).rejects.toThrow(/in progress/);
  expect(read).toHaveBeenCalledTimes(4);
  for (const request of requests) request.abort();
  await Promise.allSettled(pending);
  read.mockResolvedValue(status(records[4]!.operation, 1, 0));
  await observeRevocationEffects(test.client, scope, records[4]!, new AbortController().signal);
  expect(readRevocationEvidence(scope)).toHaveLength(4);
});

it('rejects an old-owner result after an account transition', async () => {
  retainTaskViewer('owner');
  saveShareChange('owner', target, intent);
  const test = fixture();
  let finish!: (value: ResourceRevocationStatus) => void;
  vi.spyOn(test.client.resourceRevocations!, 'status').mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  const pending = observePendingRevocation(
    test.client,
    scope,
    intent,
    intent,
    new AbortController().signal,
  );
  retainTaskViewer('other');
  finish(status());
  await expect(pending).rejects.toThrow(/account/);
  expect(sessionStorage.getItem(key)).toBeNull();
  expect(readShareChanges('owner', target)).toEqual([]);
  expect(test.writes).toEqual([]);
});
