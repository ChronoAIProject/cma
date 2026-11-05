import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { useOperationsTab } from './useOperationsTab';

afterEach(() => window.history.replaceState(null, '', '/'));
it('normalizes the legacy Usage hint without discarding filters or adding history', () => {
  window.history.replaceState(null, '', '/operations?tab=usage&owner=owner-2&range=7d');
  const entries = window.history.length;
  const { result } = renderHook(() => useOperationsTab());
  expect(result.current[0]).toBe('sandboxes');
  expect(window.location.search).toBe('?owner=owner-2&range=7d');
  expect(window.history.length).toBe(entries);
  act(() => result.current[1]('activity'));
  expect(new URLSearchParams(window.location.search).get('tab')).toBe('activity');
  expect(new URLSearchParams(window.location.search).get('owner')).toBe('owner-2');
});
it('restores an Operations tab from browser navigation', () => {
  window.history.replaceState(null, '', '/operations?tab=warm_pools');
  const { result } = renderHook(() => useOperationsTab());
  expect(result.current[0]).toBe('warm_pools');
  act(() => {
    window.history.replaceState(null, '', '/operations?tab=settings');
    window.dispatchEvent(new PopStateEvent('popstate'));
  });
  expect(result.current[0]).toBe('settings');
});
