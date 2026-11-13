import { afterEach, expect, it } from 'vitest';
import {
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
