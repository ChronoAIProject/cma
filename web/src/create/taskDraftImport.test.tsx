import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useTaskDraft } from './taskDraft';
import { emptyTaskDraft, openTaskDraftStore, taskDraftStorageKey } from './taskDraftStorage';
import { prepareHomeRequestImport } from './taskDraftImport';
import { retainTaskViewer } from './taskIdentity';
import { lifecycleStorageKey } from './lifecycleIntentStorage';
import { emptyTemplate } from './taskTemplateModel';

const owner = 'owner';
const text = 'Exact original\n请求 🧭';
beforeEach(() => {
  sessionStorage.clear();
  retainTaskViewer(owner);
});
afterEach(() => vi.restoreAllMocks());
function saved() {
  return openTaskDraftStore(owner).read();
}
function seed(draft = { ...emptyTaskDraft, text: 'Independent free draft', revision: 'original' }) {
  const store = openTaskDraftStore(owner);
  store.read();
  store.save(draft);
  return draft;
}

it('imports only exact text into an empty personal Home and verifies the saved record', () => {
  const offer = prepareHomeRequestImport(owner, text, () => true);
  expect(offer.conflict).toBe(false);
  expect(offer.commit(false)).toBe(true);
  expect(saved().draft).toMatchObject({
    text,
    machineIncluded: false,
    repositoryUrl: '',
    repositoryRef: '',
  });
  expect(saved().draft.template).toBeUndefined();
  expect(saved().recoverable).toEqual([]);
  expect(offer.commit(false)).toBe(false);
});

it.each(['free', 'template', 'repository', 'machine'] as const)(
  'requires confirmation and retains the exact replaced %s draft',
  (kind) => {
    const original = {
      ...emptyTaskDraft,
      revision: 'original',
      text: 'Independent free draft',
      ...(kind === 'template' ? { template: emptyTemplate('research') } : {}),
      ...(kind === 'repository'
        ? { repositoryUrl: 'https://github.com/example/synthetic', repositoryRef: 'topic' }
        : {}),
      ...(kind === 'machine' ? { machineIncluded: true } : {}),
    };
    seed(original);
    const before = sessionStorage.getItem(taskDraftStorageKey(owner));
    const offer = prepareHomeRequestImport(owner, text, () => true);
    expect(offer.conflict).toBe(true);
    expect(offer.commit(false)).toBe(false);
    expect(sessionStorage.getItem(taskDraftStorageKey(owner))).toBe(before);
    expect(offer.commit(true)).toBe(true);
    expect(saved().draft.text).toBe(text);
    expect(saved().recoverable).toEqual([expect.objectContaining({ draft: original })]);
  },
);

it('refuses to write behind a mounted draft owner, including a memory-only edit', () => {
  const hook = renderHook(() => useTaskDraft(owner));
  const set = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('quota');
  });
  act(() => hook.result.current.change('Memory-only text'));
  expect(() => prepareHomeRequestImport(owner, text, () => true)).toThrow('draft is open');
  expect(hook.result.current.text).toBe('Memory-only text');
  set.mockRestore();
  hook.unmount();
});

it('rechecks a newly mounted draft owner at confirmation', () => {
  seed();
  const offer = prepareHomeRequestImport(owner, text, () => true);
  const hook = renderHook(() => useTaskDraft(owner));
  expect(() => offer.commit(true)).toThrow('draft is open');
  expect(hook.result.current.text).toBe('Independent free draft');
});

it.each(['accepted', 'unknown'] as const)(
  'leaves %s request bytes and draft untouched',
  (status) => {
    seed();
    const key = lifecycleStorageKey(owner);
    const request = JSON.stringify({
      version: 1,
      intent: {
        key: 'original-key',
        body: { name: 'Original', initial_message: 'Original body' },
        ...(status === 'accepted'
          ? { receipt: { id: 'original-operation', status: 'running' } }
          : {}),
      },
    });
    sessionStorage.setItem(key, request);
    const before = sessionStorage.getItem(taskDraftStorageKey(owner));
    expect(() => prepareHomeRequestImport(owner, text, () => true)).toThrow('pending task request');
    expect(sessionStorage.getItem(key)).toBe(request);
    expect(sessionStorage.getItem(taskDraftStorageKey(owner))).toBe(before);
  },
);

it('rechecks new pending responsibility and authority at confirmation', () => {
  seed();
  let authorized = true;
  const offer = prepareHomeRequestImport(owner, text, () => authorized);
  authorized = false;
  expect(() => offer.commit(true)).toThrow('Access');
  authorized = true;
  sessionStorage.setItem(lifecycleStorageKey(owner), 'unknown original request');
  expect(() => offer.commit(true)).toThrow('pending task request');
  expect(saved().draft.text).toBe('Independent free draft');
});

it('refuses a draft revision changed after the offer', () => {
  seed();
  const offer = prepareHomeRequestImport(owner, text, () => true);
  seed({ ...emptyTaskDraft, text: 'Later change', revision: 'newer' });
  expect(() => offer.commit(true)).toThrow('Another saved draft changed');
  expect(saved().draft.text).toBe('Later change');
});

it('preserves an unreadable draft instead of importing over it', () => {
  const key = taskDraftStorageKey(owner);
  sessionStorage.setItem(key, 'unreadable draft');
  expect(() => prepareHomeRequestImport(owner, text, () => true)).toThrow();
  expect(sessionStorage.getItem(key)).toBe('unreadable draft');
});

it('does not report successful import when quota or readback verification fails', () => {
  const original = seed();
  const offer = prepareHomeRequestImport(owner, text, () => true);
  const set = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('quota');
  });
  expect(() => offer.commit(true)).toThrow('quota');
  set.mockRestore();
  expect(saved().draft).toEqual(original);
  const second = prepareHomeRequestImport(owner, text, () => true);
  const originalGet = Storage.prototype.getItem;
  let reads = 0;
  vi.spyOn(Storage.prototype, 'getItem').mockImplementation(function (this: Storage, key) {
    if (key === taskDraftStorageKey(owner) && ++reads === 2) return null;
    return originalGet.call(this, key);
  });
  expect(() => second.commit(true)).toThrow('verified');
});
