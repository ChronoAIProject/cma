import { afterEach, expect, it, vi } from 'vitest';
import {
  retainPreparationDraftOwner,
  requirePreparationDraftOwner,
} from './preparationDraftIdentity';
import {
  preparationDraftKey,
  readPreparationDraft,
  savePreparationDraft,
} from './preparationDraftStorage';

afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
  retainPreparationDraftOwner('owner-one');
});
const scope = { owner: 'owner-one', intentKey: 'command-one' };

it('preserves a confirmed same-owner draft and clears it on sign out or account change', () => {
  retainPreparationDraftOwner(scope.owner);
  const record = savePreparationDraft(scope, undefined, 'Follow-up');
  retainPreparationDraftOwner(scope.owner);
  expect(readPreparationDraft(scope)).toEqual(record);
  retainPreparationDraftOwner('owner-two');
  expect(sessionStorage.getItem(preparationDraftKey(scope))).toBeNull();
  expect(() => requirePreparationDraftOwner(scope.owner)).toThrow();
  savePreparationDraft({ ...scope, owner: 'owner-two' }, undefined, 'Another draft');
  retainPreparationDraftOwner();
  expect(sessionStorage.getItem(preparationDraftKey({ ...scope, owner: 'owner-two' }))).toBeNull();
});

it('blocks a new identity when cleanup fails, and retries cleanup before exposing drafts', () => {
  retainPreparationDraftOwner(scope.owner);
  savePreparationDraft(scope, undefined, 'Private follow-up');
  const remove = vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(() => {
    throw new Error('Storage unavailable');
  });
  retainPreparationDraftOwner();
  retainPreparationDraftOwner('owner-two');
  expect(() => requirePreparationDraftOwner('owner-two')).toThrow();
  expect(() => readPreparationDraft(scope)).toThrow();
  remove.mockRestore();
  retainPreparationDraftOwner('owner-two');
  expect(() => requirePreparationDraftOwner('owner-two')).not.toThrow();
  expect(sessionStorage.getItem(preparationDraftKey(scope))).toBeNull();
});
