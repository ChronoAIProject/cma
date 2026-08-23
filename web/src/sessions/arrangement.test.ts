import { afterEach, describe, expect, it, vi } from 'vitest';
import { DEFAULT_ARRANGEMENT, loadArrangement, saveArrangement } from './arrangement';

const KEY = 'cma:view-mode';
const LEGACY_KEY = 'chrono-bot:view-mode';

afterEach(() => {
  window.localStorage.clear();
  vi.restoreAllMocks();
});

describe('arrangement preference', () => {
  it('defaults to tabs when nothing is stored', () => {
    expect(loadArrangement()).toBe('list');
    expect(DEFAULT_ARRANGEMENT).toBe('list');
  });

  it('round-trips a stored choice', () => {
    saveArrangement('canvas');
    expect(loadArrangement()).toBe('canvas');

    saveArrangement('list');
    expect(loadArrangement()).toBe('list');
    expect(window.localStorage.getItem(LEGACY_KEY)).toBeNull();
  });

  it('migrates a valid historical choice', () => {
    window.localStorage.setItem(LEGACY_KEY, 'canvas');

    expect(loadArrangement()).toBe('canvas');
    expect(window.localStorage.getItem(KEY)).toBe('canvas');
  });

  it('migrates the historical tabs value to list', () => {
    window.localStorage.setItem(LEGACY_KEY, 'tabs');

    expect(loadArrangement()).toBe('list');
    expect(window.localStorage.getItem(KEY)).toBe('list');
  });

  it('prefers valid canonical data when both keys exist', () => {
    window.localStorage.setItem(KEY, 'list');
    window.localStorage.setItem(LEGACY_KEY, 'canvas');

    expect(loadArrangement()).toBe('list');
    expect(window.localStorage.getItem(KEY)).toBe('list');
  });

  it('falls back to valid historical data when canonical data is malformed', () => {
    window.localStorage.setItem(KEY, 'split-screen');
    window.localStorage.setItem(LEGACY_KEY, 'canvas');

    expect(loadArrangement()).toBe('canvas');
    expect(window.localStorage.getItem(KEY)).toBe('canvas');
  });

  it('ignores a value it does not recognise', () => {
    // An older or newer build, or another script on the origin, may have
    // written this. It must not become the mode.
    window.localStorage.setItem(KEY, 'split-screen');

    expect(loadArrangement()).toBe(DEFAULT_ARRANGEMENT);
  });

  it('falls back to the default when reading throws', () => {
    // Spied on the instance rather than Storage.prototype: the object behind
    // window.localStorage is not guaranteed to be a Storage in every runtime
    // the suite runs on.
    vi.spyOn(window.localStorage, 'getItem').mockImplementation(() => {
      throw new DOMException('denied', 'SecurityError');
    });

    expect(loadArrangement()).toBe(DEFAULT_ARRANGEMENT);
  });

  it('does not throw when writing fails', () => {
    vi.spyOn(window.localStorage, 'setItem').mockImplementation(() => {
      throw new DOMException('quota', 'QuotaExceededError');
    });

    expect(() => {
      saveArrangement('canvas');
    }).not.toThrow();
  });

  it('uses a historical choice even when its migration write fails', () => {
    window.localStorage.setItem(LEGACY_KEY, 'canvas');
    vi.spyOn(window.localStorage, 'setItem').mockImplementation(() => {
      throw new DOMException('quota', 'QuotaExceededError');
    });

    expect(loadArrangement()).toBe('canvas');
  });
});
