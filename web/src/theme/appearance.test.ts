import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  applyAppearance,
  effectiveAppearance,
  setAppearance,
  startAppearance,
  storedAppearance,
} from './appearance';

/**
 * The two attributes and the boot path — `docs/Web_Style.md` §3.
 *
 * The palette selects dark on `body[data-ds-dark-theme]` and carries no
 * `prefers-color-scheme` block, so every one of these is load-bearing: the
 * stylesheet cannot resolve "system", cannot notice the operating system
 * changing, and cannot apply a preference that was stored on a previous visit.
 */

/** A controllable `prefers-color-scheme` so tests are not at the mercy of CI. */
function stubSystem(dark: boolean) {
  const listeners = new Set<() => void>();
  const query = {
    matches: dark,
    addEventListener: (_: string, listener: () => void) => listeners.add(listener),
    removeEventListener: (_: string, listener: () => void) => listeners.delete(listener),
  };
  vi.stubGlobal(
    'matchMedia',
    vi.fn(() => query),
  );
  return {
    listeners,
    /** The OS flipping while the page is open. */
    flip(nowDark: boolean) {
      query.matches = nowDark;
      for (const listener of listeners) {
        listener();
      }
    },
  };
}

const isDark = () => document.body.dataset.dsDarkTheme !== undefined;

beforeEach(() => {
  window.localStorage.clear();
  delete document.documentElement.dataset.theme;
  delete document.body.dataset.dsDarkTheme;
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('applying an appearance', () => {
  it('marks the body dark for an explicit dark choice', () => {
    stubSystem(false);
    applyAppearance('dark');
    expect(isDark()).toBe(true);
    expect(document.documentElement.dataset.theme).toBe('dark');
  });

  it('clears the body mark for an explicit light choice on a dark system', () => {
    // The explicit choice wins in both directions — the failure worth pinning
    // is the one where a light choice is overridden by the machine's setting.
    stubSystem(true);
    applyAppearance('light');
    expect(isDark()).toBe(false);
    expect(document.documentElement.dataset.theme).toBe('light');
  });

  it('resolves system against the machine, and records no choice', () => {
    stubSystem(true);
    applyAppearance('system');
    // No root attribute: "system" is the absence of a choice, and the body
    // mark is the resolved value rather than the choice itself.
    expect(document.documentElement.dataset.theme).toBeUndefined();
    expect(isDark()).toBe(true);
  });

  it('falls back to light where matchMedia is unavailable', () => {
    vi.stubGlobal('matchMedia', undefined);
    expect(effectiveAppearance('system')).toBe('light');
    applyAppearance('system');
    expect(isDark()).toBe(false);
  });
});

describe('starting up', () => {
  it('applies a preference stored on a previous visit', () => {
    // The defect this exists to prevent: choosing dark, reloading, getting
    // light back because nothing ever applied what was stored.
    stubSystem(false);
    setAppearance('dark');
    delete document.body.dataset.dsDarkTheme;
    delete document.documentElement.dataset.theme;

    startAppearance();

    expect(storedAppearance()).toBe('dark');
    expect(isDark()).toBe(true);
  });

  it('follows the machine while the choice is system', () => {
    const system = stubSystem(false);
    const stop = startAppearance();
    expect(isDark()).toBe(false);

    system.flip(true);
    expect(isDark()).toBe(true);

    stop();
  });

  it('leaves an explicit choice alone when the machine changes', () => {
    const system = stubSystem(false);
    const stop = startAppearance();
    setAppearance('light');

    system.flip(true);

    expect(isDark()).toBe(false);
    stop();
  });

  it('removes its listener when stopped', () => {
    const system = stubSystem(false);
    startAppearance()();
    expect(system.listeners.size).toBe(0);
  });

  it('starts without a system query at all', () => {
    vi.stubGlobal('matchMedia', undefined);
    expect(() => startAppearance()()).not.toThrow();
  });
});
