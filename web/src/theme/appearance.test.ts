import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import {
  applyAppearance,
  effectiveAppearance,
  setAppearance,
  startAppearance,
  storedAppearance,
} from './appearance';

const STORAGE_KEY = 'cma:appearance';
const LEGACY_STORAGE_KEY = 'chrono-bot:appearance';

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
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('stored appearance', () => {
  it('writes only the canonical key', () => {
    stubSystem(false);
    setAppearance('dark');

    expect(window.localStorage.getItem(STORAGE_KEY)).toBe('dark');
    expect(window.localStorage.getItem(LEGACY_STORAGE_KEY)).toBeNull();
  });

  it('migrates a valid historical preference', () => {
    window.localStorage.setItem(LEGACY_STORAGE_KEY, 'dark');

    expect(storedAppearance()).toBe('dark');
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe('dark');
  });

  it('prefers a valid canonical preference when both exist', () => {
    window.localStorage.setItem(STORAGE_KEY, 'light');
    window.localStorage.setItem(LEGACY_STORAGE_KEY, 'dark');

    expect(storedAppearance()).toBe('light');
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe('light');
  });

  it('falls back to valid historical data when canonical data is malformed', () => {
    window.localStorage.setItem(STORAGE_KEY, 'sepia');
    window.localStorage.setItem(LEGACY_STORAGE_KEY, 'dark');

    expect(storedAppearance()).toBe('dark');
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe('dark');
  });

  it('defaults to system when neither spelling contains valid data', () => {
    window.localStorage.setItem(STORAGE_KEY, 'sepia');
    window.localStorage.setItem(LEGACY_STORAGE_KEY, 'midnight');

    expect(storedAppearance()).toBe('system');
  });

  it('uses a historical preference even when its migration write is blocked', () => {
    window.localStorage.setItem(LEGACY_STORAGE_KEY, 'dark');
    vi.spyOn(window.localStorage, 'setItem').mockImplementation(() => {
      throw new DOMException('quota', 'QuotaExceededError');
    });

    expect(storedAppearance()).toBe('dark');
  });

  it('defaults to system instead of throwing when reads are blocked', () => {
    vi.spyOn(window.localStorage, 'getItem').mockImplementation(() => {
      throw new DOMException('denied', 'SecurityError');
    });

    expect(storedAppearance()).toBe('system');
  });
});

describe('applying an appearance', () => {
  it('marks the body dark for an explicit dark choice', () => {
    stubSystem(false);
    applyAppearance('dark');
    expect(isDark()).toBe(true);
    expect(document.documentElement.style.colorScheme).toBe('dark');
  });

  it('clears the body mark for an explicit light choice on a dark system', () => {
    // The explicit choice wins in both directions — the failure worth pinning
    // is the one where a light choice is overridden by the machine's setting.
    stubSystem(true);
    applyAppearance('light');
    expect(isDark()).toBe(false);
    expect(document.documentElement.style.colorScheme).toBe('light');
  });

  it('resolves system against the machine', () => {
    stubSystem(true);
    applyAppearance('system');
    // The DOM carries what is shown, never what was chosen: "system" is a
    // preference, and the page has to be one theme or the other.
    expect(isDark()).toBe(true);
    expect(document.documentElement.style.colorScheme).toBe('dark');
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

describe('the pre-paint boot script', () => {
  /*
   * index.html decides the theme inline, before the bundle exists, so a dark
   * reader is not shown a white page for the length of a download. That means
   * the logic is written twice — and two copies of a decision drift, so this
   * runs the real one out of the real file and holds it to the same answer the
   * module gives.
   */
  const html = readFileSync(resolve(process.cwd(), 'index.html'), 'utf8');
  const inline = /<script>([\s\S]*?)<\/script>/.exec(html)?.[1] ?? '';

  /** Run the inline script against the current jsdom document. */
  function boot() {
    expect(inline.trim(), 'index.html has no inline boot script').not.toBe('');
    new Function('window', 'document', inline)(window, document);
  }

  it.each([
    ['dark', false, true],
    ['dark', true, true],
    ['light', true, false],
    ['light', false, false],
    ['system', true, true],
    ['system', false, false],
  ] as const)('agrees with the module for %s on a dark=%s machine', (choice, machine, dark) => {
    stubSystem(machine);
    setAppearance(choice);

    // What the module did, cleared — the script must reach it from nothing,
    // which is the state a fresh document is actually in.
    document.body.removeAttribute('data-ds-dark-theme');
    document.documentElement.style.colorScheme = '';

    boot();

    expect(isDark()).toBe(dark);
    expect(document.documentElement.style.colorScheme).toBe(dark ? 'dark' : 'light');
  });

  it('reads the canonical key the module writes', () => {
    // A renamed key would leave the script always answering "system" — right
    // for most people, wrong for exactly those who expressed a preference.
    setAppearance('dark');
    expect(inline).toContain(STORAGE_KEY);
    expect(inline).toContain(LEGACY_STORAGE_KEY);
  });

  it('migrates a historical preference before first paint', () => {
    stubSystem(false);
    window.localStorage.setItem(LEGACY_STORAGE_KEY, 'dark');

    boot();

    expect(isDark()).toBe(true);
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe('dark');
  });

  it('gives a valid canonical preference precedence before first paint', () => {
    stubSystem(false);
    window.localStorage.setItem(STORAGE_KEY, 'light');
    window.localStorage.setItem(LEGACY_STORAGE_KEY, 'dark');

    boot();

    expect(isDark()).toBe(false);
  });

  it('falls back from malformed canonical data before first paint', () => {
    stubSystem(false);
    window.localStorage.setItem(STORAGE_KEY, 'sepia');
    window.localStorage.setItem(LEGACY_STORAGE_KEY, 'dark');

    boot();

    expect(isDark()).toBe(true);
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe('dark');
  });

  it('still applies a historical preference when migration writes are blocked', () => {
    stubSystem(false);
    window.localStorage.setItem(LEGACY_STORAGE_KEY, 'dark');
    vi.spyOn(window.localStorage, 'setItem').mockImplementation(() => {
      throw new DOMException('quota', 'QuotaExceededError');
    });

    expect(() => boot()).not.toThrow();
    expect(isDark()).toBe(true);
  });

  it('survives storage it is not allowed to read', () => {
    // Private modes and blocked cookies both throw here. A theme preference is
    // never worth failing to render the page over.
    stubSystem(true);
    vi.spyOn(window.localStorage, 'getItem').mockImplementation(() => {
      throw new Error('blocked');
    });

    expect(() => boot()).not.toThrow();
    expect(isDark()).toBe(true);
  });
});
