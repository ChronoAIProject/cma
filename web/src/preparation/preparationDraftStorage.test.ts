import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { draftCountLimit, draftTextLimit } from '../agent/draftStorage';
import { retainPreparationDraftOwner } from './preparationDraftIdentity';
import {
  preparationDraftKey,
  preparationDraftsForAgent,
  readPreparationDraft,
  removePreparationDraft,
  savePreparationDraft,
} from './preparationDraftStorage';

const scope = { owner: 'owner-one', intentKey: 'original-command', sandboxId: 'sandbox-one' };
const binding = {
  operationId: 'operation-one',
  agentId: 'agent-one',
  sandboxId: 'sandbox-one',
  workspaceId: 'workspace-one',
  worktreeId: 'worktree-one',
};
beforeEach(() => {
  sessionStorage.clear();
  retainPreparationDraftOwner(scope.owner);
});
afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

it('keeps a separate stable key and leaves the immutable request bytes unchanged', () => {
  const original = '{"key":"original-command","body":{"initial_message":"Original task"}}';
  sessionStorage.setItem('cma:agent-creation:synthetic', original);
  const key = preparationDraftKey(scope);
  const draft = savePreparationDraft(scope, undefined, ' Follow-up\n中文 ');
  const bound = savePreparationDraft(scope, draft, draft.text, binding);
  expect(preparationDraftKey(bound.scope)).toBe(key);
  expect(readPreparationDraft(scope)).toEqual(bound);
  expect(preparationDraftsForAgent(scope.owner, binding.agentId)).toEqual([bound]);
  expect(preparationDraftsForAgent(scope.owner, 'another-agent')).toEqual([]);
  expect(sessionStorage.getItem('cma:agent-creation:synthetic')).toBe(original);
});

it('refuses a changed revision on update and removal, preserving the newer draft', () => {
  const first = savePreparationDraft(scope, undefined, 'First');
  const next = savePreparationDraft(scope, first, 'Newer');
  expect(() => savePreparationDraft(scope, first, 'Stale')).toThrow('changed elsewhere');
  expect(() => removePreparationDraft(scope, first)).toThrow('changed elsewhere');
  expect(readPreparationDraft(scope)).toEqual(next);
  removePreparationDraft(scope, next);
  expect(readPreparationDraft(scope)).toBeUndefined();
});

it('rejects wrong parent context and replacement of an existing Agent binding', () => {
  expect(() =>
    savePreparationDraft(scope, undefined, 'Draft', { ...binding, sandboxId: 'other-sandbox' }),
  ).toThrow();
  const first = savePreparationDraft(scope, undefined, 'Draft', binding);
  for (const field of ['agentId', 'operationId', 'sandboxId', 'workspaceId', 'worktreeId'] as const)
    expect(() =>
      savePreparationDraft(scope, first, 'Draft', { ...binding, [field]: 'changed' }),
    ).toThrow();
  expect(readPreparationDraft(scope)).toEqual(first);
});

it.each(['owner', 'intentKey', 'workspaceId', 'sandboxId'] as const)(
  'rejects an encoded record for a different %s',
  (field) => {
    const draft = savePreparationDraft(scope, undefined, 'Draft');
    sessionStorage.setItem(
      preparationDraftKey(scope),
      JSON.stringify({ ...draft, scope: { ...scope, [field]: 'other' } }),
    );
    expect(() => readPreparationDraft(scope)).toThrow();
  },
);

it('preserves unknown malformed storage instead of replacing it', () => {
  const key = preparationDraftKey(scope);
  sessionStorage.setItem(key, '{malformed');
  expect(() => savePreparationDraft(scope, undefined, 'Replacement')).toThrow();
  expect(sessionStorage.getItem(key)).toBe('{malformed');
});

it('bounds draft count and text without evicting another intent', () => {
  expect(() => savePreparationDraft(scope, undefined, 'a'.repeat(draftTextLimit + 1))).toThrow();
  for (let index = 0; index < draftCountLimit; index += 1)
    savePreparationDraft({ ...scope, intentKey: `intent-${index}` }, undefined, 'Keep');
  expect(() => savePreparationDraft(scope, undefined, 'Another')).toThrow();
  expect(readPreparationDraft({ ...scope, intentKey: 'intent-0' })?.text).toBe('Keep');
});

it('bounds cumulative stored bytes without discarding existing drafts', () => {
  for (let index = 0; index < 3; index += 1)
    savePreparationDraft(
      { ...scope, intentKey: `intent-${index}` },
      undefined,
      'a'.repeat(draftTextLimit),
    );
  expect(() => savePreparationDraft(scope, undefined, 'a'.repeat(draftTextLimit))).toThrow();
  expect(readPreparationDraft({ ...scope, intentKey: 'intent-0' })?.text.length).toBe(
    draftTextLimit,
  );
});

it('does not remove a saved draft on browser write failure', () => {
  const draft = savePreparationDraft(scope, undefined, 'Saved');
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new DOMException('Full', 'QuotaExceededError');
  });
  expect(() => savePreparationDraft(scope, draft, 'New in memory')).toThrow();
  expect(readPreparationDraft(scope)).toEqual(draft);
});
