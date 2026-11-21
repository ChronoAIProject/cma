import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useTaskDraft, taskDraftStorageKey, legacyTaskDraftStorageKey } from './taskDraft';
import { emptyTaskDraft } from './taskDraftStorage';

const canonical = {
  version: 3,
  ...emptyTaskDraft,
  text: 'Canonical text',
  legacyObserved: null,
  recoverable: [],
};
const legacy = { version: 2, text: 'Legacy text', revision: 'legacy-revision' };
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

it.each([
  '{broken',
  JSON.stringify({ ...canonical, version: 99 }),
  JSON.stringify({ ...canonical, legacyObserved: '{broken' }),
  JSON.stringify({ ...canonical, legacyObserved: JSON.stringify({ version: 99, text: 'Future' }) }),
  JSON.stringify({ ...canonical, recoverable: [{ id: 'copy', draft: { text: 5 } }] }),
  JSON.stringify({
    ...canonical,
    recoverable: [
      { id: 'copy', draft: emptyTaskDraft },
      { id: 'copy', draft: emptyTaskDraft },
    ],
  }),
  JSON.stringify({ ...canonical, template: { id: 'future', version: 1, fields: {} } }),
])('preserves malformed or future canonical bytes on edit or persist: %s', (raw) => {
  sessionStorage.setItem(taskDraftStorageKey('owner'), raw);
  sessionStorage.setItem(legacyTaskDraftStorageKey('owner'), JSON.stringify(legacy));
  const current = renderHook(() => useTaskDraft('owner'));
  expect(current.result.current.error).toContain('could not be read');
  act(() => current.result.current.change('Visible unsaved text'));
  act(() => {
    expect(current.result.current.persist()).toBeUndefined();
  });
  expect(current.result.current.text).toBe('Visible unsaved text');
  expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBe(raw);
  expect(sessionStorage.getItem(legacyTaskDraftStorageKey('owner'))).toBe(JSON.stringify(legacy));
});

it.each([false, true])(
  'preserves malformed legacy bytes and refuses edits (canonical exists: %s)',
  (exists) => {
    const raw = JSON.stringify({ version: 99, text: 'Unknown legacy draft' });
    if (exists) sessionStorage.setItem(taskDraftStorageKey('owner'), JSON.stringify(canonical));
    sessionStorage.setItem(legacyTaskDraftStorageKey('owner'), raw);
    const current = renderHook(() => useTaskDraft('owner'));
    expect(current.result.current.text).toBe(exists ? 'Canonical text' : '');
    expect(current.result.current.error).toContain('could not be read');
    act(() => current.result.current.change('Unsaved text'));
    expect(sessionStorage.getItem(legacyTaskDraftStorageKey('owner'))).toBe(raw);
    expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBe(
      exists ? JSON.stringify(canonical) : null,
    );
  },
);

it('deduplicates identical saved content across reordered properties and reloads', () => {
  const record = {
    version: 3,
    ...emptyTaskDraft,
    text: 'Current text',
    legacyObserved: JSON.stringify(legacy),
    recoverable: [],
  };
  sessionStorage.setItem(taskDraftStorageKey('owner'), JSON.stringify(record));
  sessionStorage.setItem(
    legacyTaskDraftStorageKey('owner'),
    JSON.stringify({
      revision: 'legacy-revision',
      ...emptyTaskDraft,
      version: 2,
      text: 'Legacy text',
    }),
  );
  const current = renderHook(() => useTaskDraft('owner'));
  expect(current.result.current.recoverable).toEqual([]);
  act(() => current.result.current.change('Next text'));
  current.unmount();
  const reopened = renderHook(() => useTaskDraft('owner'));
  expect(reopened.result.current.recoverable).toEqual([]);
  expect(reopened.result.current.text).toBe('Next text');
});

it.each([
  { owner: 'other', workspace: undefined, sandbox: undefined, origin: location.origin },
  { owner: 'owner', workspace: 'other-workspace', sandbox: undefined, origin: location.origin },
  { owner: 'owner', workspace: undefined, sandbox: 'other-sandbox', origin: location.origin },
  {
    owner: 'owner',
    workspace: undefined,
    sandbox: undefined,
    origin: 'https://other.example.test',
  },
])('isolates recovered copies from other owner/origin/task scopes: %j', (scope) => {
  sessionStorage.setItem(taskDraftStorageKey('owner'), JSON.stringify(canonical));
  sessionStorage.setItem(legacyTaskDraftStorageKey('owner'), JSON.stringify(legacy));
  const original = renderHook(() => useTaskDraft('owner'));
  expect(original.result.current.recoverable).toHaveLength(1);
  original.unmount();
  vi.stubGlobal('location', { origin: scope.origin });
  const other = renderHook(() => useTaskDraft(scope.owner, scope.workspace, scope.sandbox));
  expect(other.result.current.text).toBe('');
  expect(other.result.current.recoverable).toEqual([]);
});
