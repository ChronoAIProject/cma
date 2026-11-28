import { act, fireEvent, render, renderHook, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { testClient } from '../test/client';
import { PreparationDraft } from './PreparationDraft';
import { usePreparationDraft } from './usePreparationDraft';
import { preparationDraftKey, readPreparationDraft } from './preparationDraftStorage';
import { retainPreparationDraftOwner } from './preparationDraftIdentity';

const scope = { owner: 'owner-one', intentKey: 'original-command' };
beforeEach(() => {
  sessionStorage.clear();
  retainPreparationDraftOwner(scope.owner);
});
afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

it('restores a draft before any receipt exists without creating or sending anything', () => {
  const create = vi.fn();
  const client = testClient({ createAgentLifecycle: create });
  const original = renderHook(() => usePreparationDraft(client, scope));
  act(() => original.result.current.change(' Keep this follow-up\n中文 '));
  original.unmount();
  const restored = renderHook(() => usePreparationDraft(client, scope));
  expect(restored.result.current.text).toBe(' Keep this follow-up\n中文 ');
  expect(create).not.toHaveBeenCalled();
});

it('keeps write-failed text in memory and fences old callbacks across a client change', () => {
  const client = testClient();
  const hook = renderHook(({ active }) => usePreparationDraft(active, scope), {
    initialProps: { active: client },
  });
  const set = vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('Full');
  });
  act(() => hook.result.current.change('Memory-only follow-up'));
  const stale = hook.result.current.change;
  expect(hook.result.current.text).toBe('Memory-only follow-up');
  expect(hook.result.current.error).toMatch(/could not be saved/);
  hook.rerender({ active: testClient() });
  act(() => stale('Stale callback'));
  expect(hook.result.current.text).toBe('Memory-only follow-up');
  set.mockRestore();
  act(() => hook.result.current.retryStorage());
  expect(hook.result.current.error).toBeUndefined();
  expect(readPreparationDraft(scope)?.text).toBe('Memory-only follow-up');
});

it('blocks saved content replacement after an unreadable restore', () => {
  const key = preparationDraftKey(scope);
  sessionStorage.setItem(key, '{damaged');
  const client = testClient();
  const { result } = renderHook(() => usePreparationDraft(client, scope));
  act(() => result.current.change('New memory text'));
  expect(result.current.text).toBe('New memory text');
  expect(result.current.unread).toBe(true);
  expect(sessionStorage.getItem(key)).toBe('{damaged');
});

it('binds an exact verified lineage without moving the storage key or rebinding later', () => {
  const client = testClient();
  const { result } = renderHook(() => usePreparationDraft(client, scope));
  const binding = {
    operationId: 'operation-one',
    agentId: 'agent-one',
    sandboxId: 'sandbox-one',
    workspaceId: 'workspace-one',
    worktreeId: 'worktree-one',
  };
  act(() => result.current.change('Follow-up'));
  act(() => expect(result.current.bind(binding)).toBe(true));
  const saved = readPreparationDraft(scope);
  expect(saved?.binding).toEqual(binding);
  act(() => expect(result.current.bind({ ...binding, operationId: 'operation-two' })).toBe(false));
  expect(readPreparationDraft(scope)).toEqual(saved);
});

it('keeps unrelated intent drafts and rejects callbacks after changing owner or unmounting', () => {
  const client = testClient();
  const hook = renderHook(({ identity }) => usePreparationDraft(client, identity), {
    initialProps: { identity: scope },
  });
  act(() => hook.result.current.change('First intent'));
  const old = hook.result.current.change;
  hook.rerender({ identity: { ...scope, intentKey: 'second-command' } });
  act(() => old('Stale first intent callback'));
  expect(hook.result.current.text).toBe('');
  expect(readPreparationDraft(scope)?.text).toBe('First intent');
  act(() => hook.result.current.change('Second intent'));
  hook.rerender({ identity: { ...scope, owner: 'owner-two' } });
  expect(hook.result.current.text).toBe('');
  expect(sessionStorage.getItem(preparationDraftKey(scope))).toBeNull();
  const unmounted = hook.result.current.change;
  hook.unmount();
  act(() => unmounted('After unmount'));
  expect(readPreparationDraft({ ...scope, owner: 'owner-two' })).toBeUndefined();
});

it('has no sending action during preparation, including keyboard shortcuts and programmatic submission', () => {
  const create = vi.fn();
  const client = testClient({ createAgentLifecycle: create });
  function Host() {
    return <PreparationDraft draft={usePreparationDraft(client, scope)} />;
  }
  render(<Host />);
  const input = screen.getByLabelText('Follow-up message');
  fireEvent.change(input, { target: { value: 'Type while preparing' } });
  fireEvent.keyDown(input, { key: 'Enter' });
  fireEvent.keyDown(input, { key: 'Enter', ctrlKey: true });
  fireEvent.keyDown(input, { key: 'Enter', metaKey: true });
  fireEvent.submit(input);
  fireEvent.click(screen.getByRole('button', { name: 'Send' }));
  expect(screen.getByRole('button', { name: 'Send' })).toBeDisabled();
  expect(input).toHaveValue('Type while preparing');
  expect(readPreparationDraft(scope)?.text).toBe('Type while preparing');
  expect(create).not.toHaveBeenCalled();
});
