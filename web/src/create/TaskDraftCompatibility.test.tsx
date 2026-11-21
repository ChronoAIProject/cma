import { createHash } from 'node:crypto';
import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import {
  useTaskDraft,
  taskDraftStorageKey,
  legacyTaskDraftStorageKey,
  discardSubmittedTaskDraft,
} from './taskDraft';
import {
  useTaskDraft as usePredecessorDraft,
  discardSubmittedTaskDraft as predecessorComplete,
} from './taskDraft.v2.fixture';
import predecessorSource from './taskDraft.v2.fixture.ts?raw';
import { retainTaskViewer as predecessorViewer } from './taskIdentity.v2.fixture';
import predecessorIdentitySource from './taskIdentity.v2.fixture.ts?raw';
import type { TaskTemplate } from './taskTemplateModel';

const documentSubject = (template?: TaskTemplate) =>
  template?.id === 'document' ? template.fields.subject : undefined;

afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

it('pins the actual draft implementation from develop 4dea9b327', () => {
  expect(createHash('sha256').update(predecessorSource).digest('hex')).toBe(
    'd204737b9667078215d423ac6c9972ff6565ac7ee5a84252b14d165a3de94394',
  );
  expect(createHash('sha256').update(predecessorIdentitySource).digest('hex')).toBe(
    '422208344bdc4725acd8f7f6833bd3261d88886a25e4d60c0de8715e1cfad138',
  );
});

function keepPartialDraft() {
  const old = renderHook(() => usePredecessorDraft('owner'));
  act(() => old.result.current.change('Separate text'));
  old.unmount();
  const current = renderHook(() => useTaskDraft('owner'));
  act(() => current.result.current.selectTemplate('document'));
  act(() =>
    current.result.current.changeTemplate({
      id: 'document',
      version: 1,
      fields: { type: ' guide ', subject: '  草稿 [subject]  ', audience: '' },
    }),
  );
  act(() => current.result.current.clearTemplate());
  current.unmount();
}

it('preserves exact unsubmitted template fields across actual predecessor free-text edits', () => {
  keepPartialDraft();
  const old = renderHook(() => usePredecessorDraft('owner'));
  expect(old.result.current.text).toBe('Separate text');
  act(() => old.result.current.change('Edited by predecessor Web'));
  old.unmount();
  const restored = renderHook(() => useTaskDraft('owner'));
  expect(restored.result.current.templates?.find(({ id }) => id === 'document')?.fields).toEqual({
    type: ' guide ',
    subject: '  草稿 [subject]  ',
    audience: '',
  });
  expect(restored.result.current.text).toBe('Separate text');
  const competing = restored.result.current.recoverable.find(
    (saved) => saved.draft.text === 'Edited by predecessor Web',
  )!;
  expect(competing).toBeDefined();
  const legacy = sessionStorage.getItem(legacyTaskDraftStorageKey('owner'));
  act(() => {
    expect(restored.result.current.restoreSavedDraft(competing.id)).toBe(true);
  });
  expect(restored.result.current.text).toBe('Edited by predecessor Web');
  expect(
    restored.result.current.recoverable.some(
      (saved) => documentSubject(saved.draft.templates?.[0]) === '  草稿 [subject]  ',
    ),
  ).toBe(true);
  expect(sessionStorage.getItem(legacyTaskDraftStorageKey('owner'))).toBe(legacy);
});

it('preserves exact unsubmitted template fields across actual predecessor completion', () => {
  keepPartialDraft();
  const old = renderHook(() => usePredecessorDraft('owner'));
  let revision: string | undefined;
  act(() => {
    revision = old.result.current.persist();
  });
  old.unmount();
  predecessorComplete('owner', { name: 'New task', initial_message: 'Separate text' }, revision);
  expect(sessionStorage.getItem(legacyTaskDraftStorageKey('owner'))).toBeNull();
  const restored = renderHook(() => useTaskDraft('owner'));
  expect(restored.result.current.templates?.find(({ id }) => id === 'document')?.fields).toEqual({
    type: ' guide ',
    subject: '  草稿 [subject]  ',
    audience: '',
  });
});

it('keeps current writes separate from the actual predecessor key', () => {
  const old = renderHook(() => usePredecessorDraft('owner'));
  act(() => old.result.current.change('Legacy original'));
  old.unmount();
  const legacy = sessionStorage.getItem(legacyTaskDraftStorageKey('owner'));
  const current = renderHook(() => useTaskDraft('owner'));
  act(() => current.result.current.change('Private current edit'));
  act(() => current.result.current.selectTemplate('code'));
  expect(sessionStorage.getItem(legacyTaskDraftStorageKey('owner'))).toBe(legacy);
  const protectedDraft = sessionStorage.getItem(taskDraftStorageKey('owner'));
  current.unmount();
  const predecessor = renderHook(() => usePredecessorDraft('owner'));
  expect(predecessor.result.current.text).toBe('Legacy original');
  act(() => predecessor.result.current.change('New predecessor edit'));
  expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBe(protectedDraft);
});

it('retains competing versions of the same template through repeated predecessor round trips', () => {
  keepPartialDraft();
  for (const subject of ['Older Web first edit', 'Older Web second edit']) {
    const old = renderHook(() => usePredecessorDraft('owner'));
    act(() =>
      old.result.current.changeTemplate({
        id: 'document',
        version: 1,
        fields: { type: 'other', subject, audience: '' },
      }),
    );
    old.unmount();
    const current = renderHook(() => useTaskDraft('owner'));
    expect(documentSubject(current.result.current.templates?.[0])).toBe('  草稿 [subject]  ');
    current.unmount();
  }
  const current = renderHook(() => useTaskDraft('owner'));
  expect(
    current.result.current.recoverable.map((saved) => documentSubject(saved.draft.template)),
  ).toEqual(['Older Web first edit', 'Older Web second edit']);
  const first = current.result.current.recoverable[0]!;
  act(() => {
    expect(current.result.current.restoreSavedDraft(first.id)).toBe(true);
  });
  expect(documentSubject(current.result.current.template)).toBe('Older Web first edit');
  expect(current.result.current.revision).not.toBe(first.draft.revision);
  const revision = current.result.current.revision;
  current.unmount();
  const reopened = renderHook(() => useTaskDraft('owner'));
  expect(reopened.result.current.revision).toBe(revision);
  expect(reopened.result.current.recoverable).toHaveLength(2);
  expect(
    reopened.result.current.recoverable.some(
      (saved) => documentSubject(saved.draft.templates?.[0]) === '  草稿 [subject]  ',
    ),
  ).toBe(true);
});

it('does not resurrect an unchanged legacy source after current-version completion', () => {
  const old = renderHook(() => usePredecessorDraft('owner'));
  act(() => old.result.current.change('Submitted text'));
  old.unmount();
  const raw = sessionStorage.getItem(legacyTaskDraftStorageKey('owner'));
  const current = renderHook(() => useTaskDraft('owner'));
  let revision: string | undefined;
  act(() => {
    revision = current.result.current.persist();
  });
  act(() =>
    discardSubmittedTaskDraft(
      'owner',
      { name: 'New task', initial_message: 'Submitted text' },
      revision,
    ),
  );
  expect(current.result.current.text).toBe('');
  expect(sessionStorage.getItem(legacyTaskDraftStorageKey('owner'))).toBe(raw);
  current.unmount();
  const resumed = renderHook(() => useTaskDraft('owner'));
  expect(resumed.result.current.text).toBe('');
  expect(resumed.result.current.recoverable).toEqual([]);
});

it('rejects an old completion after an identical saved draft is restored', () => {
  keepPartialDraft();
  const old = renderHook(() => usePredecessorDraft('owner'));
  act(() => old.result.current.change('Competing task'));
  const oldRevision = old.result.current.revision;
  old.unmount();
  const current = renderHook(() => useTaskDraft('owner'));
  act(() => current.result.current.restoreSavedDraft(current.result.current.recoverable[0]!.id));
  const raw = sessionStorage.getItem(taskDraftStorageKey('owner'));
  act(() =>
    discardSubmittedTaskDraft(
      'owner',
      { name: 'New task', initial_message: 'Competing task' },
      oldRevision,
    ),
  );
  expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBe(raw);
  expect(current.result.current.text).toBe('Competing task');
});

it.each([undefined, 'another-owner'])(
  'actual predecessor cleanup removes both namespaces when the next owner is %s',
  (nextOwner) => {
    predecessorViewer('owner');
    keepPartialDraft();
    expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).not.toBeNull();
    expect(sessionStorage.getItem(legacyTaskDraftStorageKey('owner'))).not.toBeNull();
    predecessorViewer(nextOwner);
    expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBeNull();
    expect(sessionStorage.getItem(legacyTaskDraftStorageKey('owner'))).toBeNull();
  },
);

it('keeps the legacy source and visible import when the protective write fails', () => {
  const old = renderHook(() => usePredecessorDraft('owner'));
  act(() => old.result.current.change('Imported but not yet protected'));
  old.unmount();
  const legacy = sessionStorage.getItem(legacyTaskDraftStorageKey('owner'));
  const original = Storage.prototype.setItem;
  const deny = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (
    this: Storage,
    key,
    value,
  ) {
    if (key === taskDraftStorageKey('owner')) throw new DOMException('Full', 'QuotaExceededError');
    original.call(this, key, value);
  });
  const current = renderHook(() => useTaskDraft('owner'));
  expect(current.result.current.text).toBe('Imported but not yet protected');
  expect(current.result.current.error).toContain('could not be saved');
  expect(sessionStorage.getItem(legacyTaskDraftStorageKey('owner'))).toBe(legacy);
  expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBeNull();
  deny.mockRestore();
  act(() => {
    expect(current.result.current.persist()).toBeTruthy();
  });
  expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).not.toBeNull();
});

it('keeps an observed competing edit through a failed write and subsequent predecessor deletion', () => {
  keepPartialDraft();
  const current = renderHook(() => useTaskDraft('owner'));
  const old = renderHook(() => usePredecessorDraft('owner'));
  act(() => old.result.current.change('Keep this observed edit'));
  const oldRevision = old.result.current.revision;
  old.unmount();
  const original = Storage.prototype.setItem;
  const deny = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (
    this: Storage,
    key,
    value,
  ) {
    if (key === taskDraftStorageKey('owner')) throw new Error('Storage full');
    original.call(this, key, value);
  });
  act(() => current.result.current.change('My visible edit'));
  expect(current.result.current.error).toContain('could not be saved');
  predecessorComplete(
    'owner',
    { name: 'New task', initial_message: 'Keep this observed edit' },
    oldRevision,
  );
  deny.mockRestore();
  act(() => {
    expect(current.result.current.persist()).toBeTruthy();
  });
  expect(
    current.result.current.recoverable.some(
      (saved) => saved.draft.text === 'Keep this observed edit',
    ),
  ).toBe(true);
});

it('refuses stale current writers while keeping their unsaved text visible', () => {
  keepPartialDraft();
  const stale = renderHook(() => useTaskDraft('owner'));
  const fresh = renderHook(() => useTaskDraft('owner'));
  act(() => fresh.result.current.change('Committed in another editor'));
  const saved = sessionStorage.getItem(taskDraftStorageKey('owner'));
  act(() => stale.result.current.change('Unsaved text to copy'));
  expect(stale.result.current.text).toBe('Unsaved text to copy');
  expect(stale.result.current.error).toContain('Another saved draft changed');
  expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBe(saved);
});

it('refuses recovery when storage fails without discarding either draft', () => {
  keepPartialDraft();
  const old = renderHook(() => usePredecessorDraft('owner'));
  act(() => old.result.current.change('Recovery target'));
  old.unmount();
  const current = renderHook(() => useTaskDraft('owner'));
  const raw = sessionStorage.getItem(taskDraftStorageKey('owner'));
  const snapshot = current.result.current.recoverable[0]!;
  const deny = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('Full');
  });
  act(() => {
    expect(current.result.current.restoreSavedDraft(snapshot.id)).toBe(false);
  });
  expect(current.result.current.text).toBe('Separate text');
  expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBe(raw);
  deny.mockRestore();
  act(() => {
    expect(current.result.current.restoreSavedDraft(snapshot.id)).toBe(true);
  });
  expect(current.result.current.text).toBe('Recovery target');
  expect(
    current.result.current.recoverable.some((saved) => saved.draft.text === 'Separate text'),
  ).toBe(true);
});

it.each(['before', 'after'])(
  'keeps identical newly authored predecessor text %s current completion',
  (when) => {
    const predecessor = renderHook(() => usePredecessorDraft('owner'));
    act(() => predecessor.result.current.change('Identical task'));
    predecessor.unmount();
    const current = renderHook(() => useTaskDraft('owner'));
    let submitted: string | undefined;
    act(() => {
      submitted = current.result.current.persist();
    });
    const finish = () =>
      act(() =>
        discardSubmittedTaskDraft(
          'owner',
          { name: 'New task', initial_message: 'Identical task' },
          submitted,
        ),
      );
    if (when === 'after') finish();
    const next = renderHook(() => usePredecessorDraft('owner'));
    act(() => next.result.current.change('Intervening edit'));
    act(() => next.result.current.change('Identical task'));
    const nextRevision = next.result.current.revision;
    next.unmount();
    if (when === 'before') finish();
    current.unmount();
    const reopened = renderHook(() => useTaskDraft('owner'));
    expect(reopened.result.current.text).toBe('');
    expect(reopened.result.current.recoverable).toEqual([
      expect.objectContaining({
        draft: expect.objectContaining({ text: 'Identical task', revision: nextRevision }),
      }),
    ]);
  },
);

it('refuses a stale recovery without replacing the latest current draft', () => {
  keepPartialDraft();
  const old = renderHook(() => usePredecessorDraft('owner'));
  act(() => old.result.current.change('Recovery target'));
  old.unmount();
  const stale = renderHook(() => useTaskDraft('owner'));
  const target = stale.result.current.recoverable[0]!;
  const fresh = renderHook(() => useTaskDraft('owner'));
  act(() => fresh.result.current.change('Latest current draft'));
  const saved = sessionStorage.getItem(taskDraftStorageKey('owner'));
  act(() => {
    expect(stale.result.current.restoreSavedDraft(target.id)).toBe(false);
  });
  expect(stale.result.current.text).toBe('Separate text');
  expect(stale.result.current.error).toContain('Another saved draft changed');
  expect(sessionStorage.getItem(taskDraftStorageKey('owner'))).toBe(saved);
});

it('captures a new predecessor edit that arrives between reading and restoring a saved draft', () => {
  keepPartialDraft();
  const old = renderHook(() => usePredecessorDraft('owner'));
  act(() => old.result.current.change('First recovery target'));
  const current = renderHook(() => useTaskDraft('owner'));
  const first = current.result.current.recoverable[0]!;
  act(() => old.result.current.change('Later predecessor edit'));
  act(() => {
    expect(current.result.current.restoreSavedDraft(first.id)).toBe(true);
  });
  expect(current.result.current.text).toBe('First recovery target');
  expect(current.result.current.recoverable.map((saved) => saved.draft.text).sort()).toEqual([
    'Later predecessor edit',
    'Separate text',
  ]);
});
