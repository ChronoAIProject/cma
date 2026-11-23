import { restoreDeletions as predecessorRestore } from './test/deletionRecoveryPredecessor';
import { afterEach, expect, it } from 'vitest';
import {
  acknowledgeRetainedDeletion,
  deletionStorageKey,
  forgetDeletion,
  persistDeletion,
  restoreDeletion,
  restoreDeletions,
} from './deletionRecovery';
import type { DeletionRequest } from './deletion';

afterEach(() => localStorage.clear());
const first: DeletionRequest = {
  action: { kind: 'delete_agent', agentId: 'first' },
  key: 'first-key',
  accepted: true,
};
const second: DeletionRequest = {
  action: { kind: 'delete_agent', agentId: 'second' },
  key: 'second-key',
  accepted: false,
};

it('keeps independent requests, prioritizes uncertainty, and removes only the completed key', () => {
  persistDeletion('owner', first);
  persistDeletion('owner', second);
  expect(restoreDeletions('owner')).toEqual(expect.arrayContaining([first, second]));
  expect(restoreDeletion('owner')).toEqual(second);
  forgetDeletion('owner', first.key);
  expect(restoreDeletions('owner')).toEqual([second]);
  expect(restoreDeletions('another-owner')).toEqual([]);
});

it('retains legacy uncertainty alongside new requests and updates only that request', () => {
  localStorage.setItem(deletionStorageKey('owner'), JSON.stringify(second));
  persistDeletion('owner', first);
  expect(restoreDeletions('owner')).toHaveLength(2);
  persistDeletion('owner', { ...second, accepted: true });
  expect(JSON.parse(localStorage.getItem(deletionStorageKey('owner'))!)).toEqual({
    ...second,
    accepted: true,
  });
  expect(restoreDeletions('owner')).toHaveLength(2);
  expect(restoreDeletions('owner').every((request) => request.accepted)).toBe(true);
});

it('rejects malformed data and isolates viewer names that contain separators', () => {
  for (const value of [
    '{broken',
    '[]',
    'null',
    JSON.stringify({ ...first, key: '' }),
    JSON.stringify({ ...first, action: { kind: 'delete_agent', agentId: '../other' } }),
  ]) {
    localStorage.setItem(deletionStorageKey('owner'), value);
    expect(restoreDeletions('owner')).toEqual([]);
  }
  persistDeletion('owner:request', first);
  persistDeletion('owner', second);
  expect(restoreDeletions('owner')).toEqual([second]);
  expect(restoreDeletions('owner:request')).toEqual([first]);
});

it('restores distinct Stop and Delete keys but rejects a malformed combined intent', () => {
  const combined: DeletionRequest = {
    ...second,
    stop: {
      key: 'original-stop',
      phase: 'accepted',
      sandboxId: 'sbx_1',
      workspaceId: 'ws_1',
      worktreeId: 'wkt_1',
    },
  };
  persistDeletion('owner', combined);
  expect(restoreDeletion('owner')).toEqual(combined);
  for (const invalid of [
    { ...combined, stop: null },
    { ...combined, deleteSent: 'no' },
    { ...combined, stop: { ...combined.stop, key: second.key } },
    { ...combined, stop: { ...combined.stop, phase: 'done' } },
    { ...combined, stop: { ...combined.stop, sandboxId: '../other' } },
    { ...combined, stop: { ...combined.stop, worktreeId: undefined } },
    { ...combined, action: { kind: 'delete_sandbox', sandboxId: 'sbx_1' } },
  ]) {
    localStorage.clear();
    localStorage.setItem(
      `cma:stop-delete:v1:owner:${second.key}`,
      JSON.stringify({ version: 1, request: invalid }),
    );
    expect(restoreDeletion('owner')).toBeUndefined();
  }
});

const combined: DeletionRequest = {
  ...second,
  stop: {
    key: 'stop-key',
    phase: 'blocked',
    sandboxId: 'sbx_1',
    workspaceId: 'ws_1',
    worktreeId: 'wkt_1',
  },
};
it('keeps combined intent invisible to the actual predecessor reader while both restore legacy Delete', () => {
  persistDeletion('owner', combined);
  expect(predecessorRestore('owner')).toEqual([]);
  expect(localStorage.getItem(deletionStorageKey('owner'))).toBeNull();
  persistDeletion('owner', first);
  expect(predecessorRestore('owner')).toEqual([first]);
  expect(restoreDeletions('owner')).toEqual(expect.arrayContaining([first, combined]));
  forgetDeletion('owner', combined.key);
  expect(restoreDeletions('owner')).toEqual([first]);
});
it.each([
  { version: 2, request: combined },
  { request: combined },
  { version: 1, request: second },
  { version: 1, request: { ...combined, key: 'mismatched-key' } },
  { version: 1, request: { ...combined, stop: { ...combined.stop, workspaceId: undefined } } },
  { version: 1, request: { ...combined, stop: { ...combined.stop, key: 'k'.repeat(129) } } },
])('refuses invalid combined envelope without falling back to direct Delete: %j', (value) => {
  localStorage.setItem(`cma:stop-delete:v1:owner:${second.key}`, JSON.stringify(value));
  expect(restoreDeletions('owner')).toEqual([]);
  expect(predecessorRestore('owner')).toEqual([]);
});
it('never reads combined fields from the legacy namespace', () => {
  localStorage.setItem(deletionStorageKey('owner'), JSON.stringify(combined));
  expect(restoreDeletions('owner')).toEqual([]);
});

it.each([false, true])(
  'retains the combined Delete dispatch marker %s without exposing it to the predecessor',
  (deleteSent) => {
    const recorded = { ...combined, deleteSent };
    persistDeletion('owner', recorded);
    expect(restoreDeletion('owner')).toEqual(recorded);
    expect(predecessorRestore('owner')).toEqual([]);
  },
);

it.each([first, combined])(
  'never downgrades an accepted receipt with a stale write: %j',
  (request) => {
    const accepted = { ...request, accepted: true, deleteSent: true };
    persistDeletion('owner', accepted);
    const saved = persistDeletion('owner', { ...request, accepted: false, deleteSent: false });
    expect(restoreDeletions('owner')).toEqual([accepted]);
    expect(saved).toEqual(accepted);
  },
);

it.each(['viewer', 'action', 'key', 'stop', 'sandboxId', 'workspaceId', 'worktreeId'] as const)(
  'does not borrow late acceptance from a different %s',
  (difference) => {
    const retained = { ...combined, deleteSent: true };
    persistDeletion('owner', retained);
    const sent: DeletionRequest =
      difference === 'action'
        ? { ...retained, action: { kind: 'delete_agent', agentId: 'another' } }
        : difference === 'key'
          ? { ...retained, key: 'another-delete' }
          : difference === 'stop'
            ? { ...retained, stop: { ...retained.stop!, key: 'another-stop' } }
            : difference === 'viewer'
              ? retained
              : { ...retained, stop: { ...retained.stop!, [difference]: 'another-parent' } };
    acknowledgeRetainedDeletion(difference === 'viewer' ? 'another-owner' : 'owner', sent);
    expect(restoreDeletions('owner')).toEqual([retained]);
    expect(restoreDeletions('another-owner')).toEqual([]);
  },
);
