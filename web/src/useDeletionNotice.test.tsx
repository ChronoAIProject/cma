import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { DeletionRequest } from './deletion';
import { useDeletionNotice } from './useDeletionNotice';

const first: DeletionRequest = {
  key: 'first',
  accepted: true,
  action: { kind: 'delete_agent', agentId: 'a' },
};
const second: DeletionRequest = {
  key: 'second',
  accepted: false,
  action: { kind: 'delete_sandbox', sandboxId: 'b' },
};
const environment = { view: '/sandboxes/b', environment: true };
afterEach(() => vi.restoreAllMocks());

it('keeps the dismissed set hidden through updates, retries, partial settlement and remount', () => {
  const view = renderHook(({ requests }) => useDeletionNotice('owner', requests, environment), {
    initialProps: { requests: [first, second] },
  });
  act(() => view.result.current.dismiss());
  expect(view.result.current.central).toBe(false);
  view.rerender({
    requests: [
      { ...second, accepted: true },
      { ...first, deleteSent: true },
    ],
  });
  act(() => view.result.current.started(first, view.result.current.origin));
  expect(view.result.current.central).toBe(false);
  view.rerender({
    requests: [
      {
        ...first,
        stop: {
          key: 'stop-key',
          phase: 'accepted',
          sandboxId: 'sandbox',
          workspaceId: 'workspace',
          worktreeId: 'worktree',
        },
      },
      second,
    ],
  });
  expect(view.result.current.central).toBe(false);
  view.rerender({ requests: [second] });
  expect(view.result.current.central).toBe(false);
  view.unmount();
  const reloaded = renderHook(({ requests }) => useDeletionNotice('owner', requests, environment), {
    initialProps: { requests: [second] },
  });
  expect(reloaded.result.current.central).toBe(false);
  reloaded.rerender({ requests: [first, second] });
  expect(reloaded.result.current.central).toBe(false);
  reloaded.rerender({ requests: [second, { ...first, key: 'new-intent' }] });
  expect(reloaded.result.current.central).toBe(true);
});

it('announces own new requests only in their current view, including delayed acceptance', () => {
  const view = renderHook(
    ({ requests, context }) => useDeletionNotice('owner', requests, context),
    { initialProps: { requests: [first], context: { view: '/agents/a', environment: false } } },
  );
  expect(view.result.current.central).toBe(false);
  expect(view.result.current.environment).toBe(true);
  const origin = view.result.current.origin;
  act(() => view.result.current.started(second, origin));
  expect(view.result.current.central).toBe(false);
  view.rerender({ requests: [first, second], context: { view: '/agents/a', environment: false } });
  expect(view.result.current.central).toBe(true);
  expect(view.result.current.environment).toBe(false);
  view.rerender({ requests: [first, second], context: { view: '/', environment: false } });
  act(() => view.result.current.started({ ...second, accepted: true }, origin));
  expect(view.result.current.central).toBe(false);
  view.rerender({ requests: [first, second], context: { view: '/agents/a', environment: false } });
  expect(view.result.current.central).toBe(false);
});

it('isolates users and resets the open list and old origin on owner changes', () => {
  const view = renderHook(({ viewer }) => useDeletionNotice(viewer, [first], environment), {
    initialProps: { viewer: 'first-owner' },
  });
  const origin = view.result.current.origin;
  act(() => {
    view.result.current.dismiss();
    view.result.current.openList();
  });
  view.rerender({ viewer: 'second-owner' });
  expect(view.result.current.central).toBe(true);
  expect(view.result.current.open).toBe(false);
  act(() => view.result.current.started(first, origin));
  view.rerender({ viewer: 'first-owner' });
  expect(view.result.current.central).toBe(false);
});

it('keeps the progress list closed after the last request settles and a new request appears', () => {
  const view = renderHook(({ requests }) => useDeletionNotice('owner', requests, environment), {
    initialProps: { requests: [first] },
  });
  act(() => view.result.current.openList());
  expect(view.result.current.open).toBe(true);
  view.rerender({ requests: [] });
  expect(view.result.current.open).toBe(false);
  view.rerender({ requests: [second] });
  expect(view.result.current.open).toBe(false);
});

it('remembers dismissal in memory when session storage is unavailable', () => {
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('Storage unavailable');
  });
  const view = renderHook(() => useDeletionNotice('blocked-storage-owner', [first], environment));
  act(() => view.result.current.dismiss());
  view.unmount();
  const reloaded = renderHook(() =>
    useDeletionNotice('blocked-storage-owner', [first], environment),
  );
  expect(reloaded.result.current.central).toBe(false);
  expect(reloaded.result.current.environment).toBe(false);
});

it('treats malformed session preferences as empty without changing requests', () => {
  sessionStorage.setItem('cma:deletion-notice:v1:owner', '{broken');
  const view = renderHook(() => useDeletionNotice('owner', [first], environment));
  expect(view.result.current.central).toBe(true);
  act(() => view.result.current.dismiss());
  expect(view.result.current.central).toBe(false);
});

it('starts a new browser session without the previous dismissal', () => {
  const view = renderHook(() => useDeletionNotice('new-session-owner', [first], environment));
  act(() => view.result.current.dismiss());
  view.unmount();
  sessionStorage.clear();
  const fresh = renderHook(() => useDeletionNotice('new-session-owner', [first], environment));
  expect(fresh.result.current.central).toBe(true);
});
