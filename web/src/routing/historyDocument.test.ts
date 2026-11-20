import { afterEach, expect, it, vi } from 'vitest';
import { prepareHistoryDocument } from './historyDocument';

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
  sessionStorage.clear();
});

it('hands off the exact URL and existing state before mounting in a browser without an index', () => {
  vi.stubGlobal('navigation', undefined);
  const state = { legacy: 'retained', usageBackground: '/artifacts', nested: { page: 2 } };
  history.replaceState(state, '', '/usage?label=two%20words&label=again#retained');
  const url = location.href;
  const length = history.length;
  const replace = vi.fn();
  expect(prepareHistoryDocument(replace)).toBe(false);
  expect(replace).toHaveBeenCalledOnce();
  expect(history.state).toEqual(state);
  expect(location.href).toBe(url);
  expect(history.length).toBe(length);
  const destination = replace.mock.calls[0]![0] as string;
  expect(new URL(destination).searchParams.get('cmaHistoryDocument')).toBeTruthy();

  // Cross-document replacement loses history.state; bootstrap must restore the handoff.
  history.replaceState(null, '', destination);
  expect(prepareHistoryDocument(replace)).toBe(true);
  expect(location.href).toBe(url);
  expect(history.state).toEqual({
    ...state,
    cmaNavigation: { session: expect.any(String), index: 0, documentBoundary: true },
  });
  expect(sessionStorage.length).toBe(0);
  expect(prepareHistoryDocument(replace)).toBe(true);
  expect(replace).toHaveBeenCalledOnce();
});

it('does not replace a document whose browser exposes its history index', () => {
  vi.stubGlobal('navigation', { currentEntry: { index: 2 } });
  history.replaceState({ legacy: true }, '', '/artifacts');
  const replace = vi.fn();
  expect(prepareHistoryDocument(replace)).toBe(true);
  expect(replace).not.toHaveBeenCalled();
  expect(sessionStorage.length).toBe(0);
  expect(history.state).toEqual({ legacy: true });
});

it('does not navigate or mount if the existing history state cannot be retained', () => {
  vi.stubGlobal('navigation', undefined);
  history.replaceState({ legacy: true }, '', '/artifacts');
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new DOMException('Storage unavailable', 'SecurityError');
  });
  const replace = vi.fn();
  expect(() => prepareHistoryDocument(replace)).toThrow('Storage unavailable');
  expect(replace).not.toHaveBeenCalled();
  expect(history.state).toEqual({ legacy: true });
});

it('refuses to silently alter a non-JSON legacy state during the handoff', () => {
  vi.stubGlobal('navigation', undefined);
  const state = { recordedAt: new Date('2026-01-01T00:00:00Z') };
  history.replaceState(state, '', '/artifacts');
  const replace = vi.fn();
  expect(() => prepareHistoryDocument(replace)).toThrow('History state cannot be retained');
  expect(replace).not.toHaveBeenCalled();
  expect(history.state).toEqual(state);
  expect(sessionStorage.length).toBe(0);
});
