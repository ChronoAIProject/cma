import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useNavigationFilterState } from './navigationFilterState';

const key = (viewer = 'viewer-one', scope = '', origin = location.origin, version = 1) =>
  `cma:navigation-filters:v${version}:${JSON.stringify([origin, viewer, scope])}`;
const defaults = { agent: 'all', sandbox: 'all' };
afterEach(() => vi.restoreAllMocks());

it('restores only the two enums within the same tab, viewer, origin and scope', () => {
  const first = renderHook(() => useNavigationFilterState('viewer-one', ''));
  act(() => first.result.current.select('sandbox', 'installing'));
  act(() => first.result.current.select('agent', 'working'));
  expect(JSON.parse(sessionStorage.getItem(key())!)).toEqual({
    agent: 'working',
    sandbox: 'installing',
  });
  first.unmount();
  const second = renderHook(({ viewer, scope }) => useNavigationFilterState(viewer, scope), {
    initialProps: { viewer: 'viewer-one', scope: '' },
  });
  expect(second.result.current.statuses).toEqual({ agent: 'working', sandbox: 'installing' });
  second.rerender({ viewer: 'viewer-two', scope: '' });
  expect(second.result.current.statuses).toEqual(defaults);
  second.rerender({ viewer: 'viewer-one', scope: 'shared' });
  expect(second.result.current.statuses).toEqual(defaults);
  act(() => second.result.current.select('sandbox', 'provisioning'));
  second.rerender({ viewer: 'viewer-one', scope: 'organization:research' });
  expect(second.result.current.statuses).toEqual(defaults);
  second.rerender({ viewer: 'viewer-one', scope: 'shared' });
  expect(second.result.current.statuses.sandbox).toBe('provisioning');
  second.rerender({ viewer: 'viewer-one', scope: '' });
  expect(second.result.current.statuses).toEqual({ agent: 'working', sandbox: 'installing' });
});

it.each([
  'broken json',
  'null',
  '[]',
  '{}',
  '{"agent":"working","sandbox":"removed"}',
  '{"agent":"installing","sandbox":"working"}',
  '{"agent":"working","sandbox":"installing","name":"private title"}',
])('falls back to All statuses for invalid stored preferences: %s', (raw) => {
  sessionStorage.setItem(key(), raw);
  const hook = renderHook(() => useNavigationFilterState('viewer-one', ''));
  expect(hook.result.current.statuses).toEqual(defaults);
  expect(hook.result.current.unavailable).toBe(false);
});

it('ignores old versions and preferences from another origin', () => {
  sessionStorage.setItem(
    key('viewer-one', '', location.origin, 0),
    JSON.stringify({ agent: 'working', sandbox: 'installing' }),
  );
  sessionStorage.setItem(
    key('viewer-one', '', 'https://other.invalid'),
    JSON.stringify({ agent: 'working', sandbox: 'installing' }),
  );
  const hook = renderHook(() => useNavigationFilterState('viewer-one', ''));
  expect(hook.result.current.statuses).toEqual(defaults);
});

it('restores runtime and access updates as separate saved status values', () => {
  const hook = renderHook(() => useNavigationFilterState('viewer-one', ''));
  act(() => hook.result.current.select('sandbox', 'rotating'));
  hook.unmount();
  const restored = renderHook(() => useNavigationFilterState('viewer-one', ''));
  expect(restored.result.current.statuses.sandbox).toBe('rotating');
  act(() => restored.result.current.select('sandbox', 'credential_rotating'));
  expect(JSON.parse(sessionStorage.getItem(key())!).sandbox).toBe('credential_rotating');
});

it('clears both views and the Personal destination without changing another scope', () => {
  for (const scope of ['', 'shared', 'organization:research'])
    sessionStorage.setItem(
      key('viewer-one', scope),
      JSON.stringify({ agent: 'working', sandbox: 'installing' }),
    );
  const hook = renderHook(() => useNavigationFilterState('viewer-one', 'shared'));
  act(() => hook.result.current.clear());
  expect(hook.result.current.statuses).toEqual(defaults);
  expect(sessionStorage.getItem(key())).toBeNull();
  expect(sessionStorage.getItem(key('viewer-one', 'shared'))).toBeNull();
  expect(sessionStorage.getItem(key('viewer-one', 'organization:research'))).not.toBeNull();
  hook.unmount();
  const restored = renderHook(() => useNavigationFilterState('viewer-one', 'shared'));
  expect(restored.result.current.statuses).toEqual(defaults);
});

it.each(['getItem', 'setItem', 'removeItem'] as const)(
  'keeps usable in-memory filters if storage %s throws',
  (method) => {
    vi.spyOn(Storage.prototype, method).mockImplementation(() => {
      throw new Error('Unavailable');
    });
    const hook = renderHook(({ scope }) => useNavigationFilterState('viewer-one', scope), {
      initialProps: { scope: '' },
    });
    if (method === 'getItem') expect(hook.result.current.unavailable).toBe(true);
    act(() => hook.result.current.select('sandbox', 'installing'));
    expect(hook.result.current.statuses.sandbox).toBe('installing');
    if (method === 'setItem') expect(hook.result.current.unavailable).toBe(true);
    act(() => hook.result.current.select('sandbox', 'invented'));
    expect(hook.result.current.statuses.sandbox).toBe('installing');
    hook.rerender({ scope: 'shared' });
    expect(hook.result.current.statuses).toEqual(defaults);
    hook.rerender({ scope: '' });
    expect(hook.result.current.statuses.sandbox).toBe('installing');
    act(() => hook.result.current.clear());
    expect(hook.result.current.statuses).toEqual(defaults);
    if (method === 'removeItem') expect(hook.result.current.unavailable).toBe(true);
  },
);
