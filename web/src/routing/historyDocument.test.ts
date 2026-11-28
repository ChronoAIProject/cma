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

it('preserves the replayed fields of a JSON array in legacy browser state', () => {
  vi.stubGlobal('navigation', undefined);
  history.replaceState(['retained'], '', '/artifacts');
  const replace = vi.fn();
  expect(prepareHistoryDocument(replace)).toBe(false);
  history.replaceState(null, '', replace.mock.calls[0]![0] as string);
  expect(prepareHistoryDocument(replace)).toBe(true);
  expect(history.state).toMatchObject({ 0: 'retained', cmaNavigation: { documentBoundary: true } });
});

it.each([
  '{',
  JSON.stringify({ state: [], destination: 'https://example.invalid' }),
  JSON.stringify({ state: null, destination: 'invalid URL' }),
  JSON.stringify({ state: { ignored: true }, destination: 'https://example.invalid' }),
])('keeps a tab with a native history index usable after a rejected handoff', (raw) => {
  vi.stubGlobal('navigation', { currentEntry: { index: 2 } });
  const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
  const replace = vi.fn();
  const key = 'cma.history-document.rejected';
  sessionStorage.setItem(key, raw);
  const state = { current: true };
  history.replaceState(
    state,
    '',
    '/usage?filter=one&filter=two&cmaHistoryDocument=rejected#target',
  );
  const length = history.length;

  expect(prepareHistoryDocument(replace)).toBe(true);
  expect(location.search).toBe('?filter=one&filter=two');
  expect(location.hash).toBe('#target');
  expect(history.state).toEqual(state);
  expect(history.length).toBe(length);
  expect(replace).not.toHaveBeenCalled();
  expect(prepareHistoryDocument(replace)).toBe(true);
  expect(warn).toHaveBeenCalledOnce();
  expect(warn).toHaveBeenCalledWith('CMA ignored an unavailable or invalid history handoff.');
  expect(sessionStorage.getItem(key)).toBe(raw);
});

it('rebuilds a legacy document boundary from current state while retaining the rejected value', () => {
  vi.stubGlobal('navigation', undefined);
  const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
  const replace = vi.fn();
  const key = 'cma.history-document.rejected';
  const rejected = JSON.stringify({
    state: { wrong: true },
    destination: location.origin + '/other',
  });
  sessionStorage.setItem(key, rejected);
  sessionStorage.setItem('cma.command-journal', 'keep');
  history.replaceState(
    { current: true },
    '',
    '/usage?filter=one&cmaHistoryDocument=rejected#target',
  );
  const length = history.length;

  expect(prepareHistoryDocument(replace)).toBe(false);
  const fresh = new URL(replace.mock.calls[0]![0] as string);
  expect(fresh.pathname).toBe('/usage');
  expect(fresh.searchParams.get('cmaHistoryDocument')).not.toBe('rejected');
  expect(fresh.searchParams.get('filter')).toBe('one');
  expect(fresh.hash).toBe('#target');
  history.replaceState(null, '', fresh.href);
  expect(prepareHistoryDocument(replace)).toBe(true);
  expect(history.state).toMatchObject({ current: true, cmaNavigation: { documentBoundary: true } });
  expect(location.search).toBe('?filter=one');
  expect(location.hash).toBe('#target');
  expect(history.length).toBe(length);
  expect(prepareHistoryDocument(replace)).toBe(true);
  expect(replace).toHaveBeenCalledOnce();
  expect(warn).toHaveBeenCalledOnce();
  expect(sessionStorage.length).toBe(2);
  expect(sessionStorage.getItem(key)).toBe(rejected);
  expect(sessionStorage.getItem('cma.command-journal')).toBe('keep');
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
