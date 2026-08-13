import { afterEach, describe, expect, it, vi } from 'vitest';
import { DEFAULT_VIEW_MODE, loadViewMode, saveViewMode } from './mode';

const KEY = 'chrono-bot:view-mode';

afterEach(() => {
  window.localStorage.clear();
  vi.restoreAllMocks();
});

describe('view mode preference', () => {
  it('defaults to tabs when nothing is stored', () => {
    expect(loadViewMode()).toBe('tabs');
    expect(DEFAULT_VIEW_MODE).toBe('tabs');
  });

  it('round-trips a stored choice', () => {
    saveViewMode('canvas');
    expect(loadViewMode()).toBe('canvas');

    saveViewMode('tabs');
    expect(loadViewMode()).toBe('tabs');
  });

  it('ignores a value it does not recognise', () => {
    // An older or newer build, or another script on the origin, may have
    // written this. It must not become the mode.
    window.localStorage.setItem(KEY, 'split-screen');

    expect(loadViewMode()).toBe(DEFAULT_VIEW_MODE);
  });

  it('falls back to the default when reading throws', () => {
    // Spied on the instance rather than Storage.prototype: the object behind
    // window.localStorage is not guaranteed to be a Storage in every runtime
    // the suite runs on.
    vi.spyOn(window.localStorage, 'getItem').mockImplementation(() => {
      throw new DOMException('denied', 'SecurityError');
    });

    expect(loadViewMode()).toBe(DEFAULT_VIEW_MODE);
  });

  it('does not throw when writing fails', () => {
    vi.spyOn(window.localStorage, 'setItem').mockImplementation(() => {
      throw new DOMException('quota', 'QuotaExceededError');
    });

    expect(() => {
      saveViewMode('canvas');
    }).not.toThrow();
  });
});
