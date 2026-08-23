import { afterEach, describe, expect, it, vi } from 'vitest';
import { allThemes, resolveTheme, storedThemeId, storeThemeId, themesFor } from './themes';

const STORAGE_KEY = 'cma:terminal-theme';
const LEGACY_STORAGE_KEY = 'chrono-bot:terminal-theme';

afterEach(() => {
  window.localStorage.clear();
  vi.restoreAllMocks();
});

/**
 * The rule worth protecting is that a palette never fights the page it sits
 * on. Everything else here guards the transcription against drift.
 */

describe('the terminal palettes', () => {
  it('offers themes for both appearances', () => {
    expect(themesFor('dark').length).toBeGreaterThan(1);
    expect(themesFor('light').length).toBeGreaterThan(1);
  });

  it("puts the product's own palette first for each appearance", () => {
    // It is the fallback by construction, so its position is load-bearing
    // rather than cosmetic.
    expect(themesFor('dark')[0]?.label).toBe('CMA');
    expect(themesFor('light')[0]?.label).toBe('CMA');
    expect(themesFor('dark')[0]?.id).toBe('cma-dark');
    expect(themesFor('light')[0]?.id).toBe('cma-light');
  });

  it('never offers a dark palette for a light page', () => {
    // A dark rectangle in the middle of a light page is the failure this
    // filtering exists to prevent.
    for (const theme of themesFor('light')) {
      expect(theme.appearance).toBe('light');
    }
    for (const theme of themesFor('dark')) {
      expect(theme.appearance).toBe('dark');
    }
  });

  it('gives every palette a complete set of colours', () => {
    // xterm silently falls back for a missing slot, so an incomplete
    // transcription would show up as a colour that is subtly wrong rather
    // than as an error.
    const required = [
      'background',
      'foreground',
      'cursor',
      'black',
      'red',
      'green',
      'yellow',
      'blue',
      'magenta',
      'cyan',
      'white',
      'brightBlack',
      'brightRed',
      'brightGreen',
      'brightYellow',
      'brightBlue',
      'brightMagenta',
      'brightCyan',
      'brightWhite',
    ] as const;
    for (const t of allThemes()) {
      for (const slot of required) {
        expect(t.theme[slot], `${t.label} is missing ${slot}`).toBeDefined();
      }
    }
  });

  it('gives every palette a distinct id', () => {
    const ids = allThemes().map((t) => t.id);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it('honours a stored choice that suits the page', () => {
    expect(resolveTheme('dark', 'dracula').id).toBe('dracula');
  });

  it('ignores a stored choice that does not suit the page', () => {
    // Kept rather than cleared: switching the appearance back restores it.
    expect(resolveTheme('light', 'dracula').appearance).toBe('light');
    expect(resolveTheme('light', 'dracula').label).toBe('CMA');
  });

  it('falls back when nothing has been chosen', () => {
    expect(resolveTheme('dark', null).label).toBe('CMA');
    expect(resolveTheme('dark', 'no-such-theme').label).toBe('CMA');
  });
});

describe('stored terminal palette', () => {
  it('writes only the canonical storage key', () => {
    storeThemeId('dracula');

    expect(window.localStorage.getItem(STORAGE_KEY)).toBe('dracula');
    expect(window.localStorage.getItem(LEGACY_STORAGE_KEY)).toBeNull();
  });

  it('migrates a valid historical palette', () => {
    window.localStorage.setItem(LEGACY_STORAGE_KEY, 'dracula');

    expect(storedThemeId()).toBe('dracula');
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe('dracula');
  });

  it.each([
    ['chronobot-dark', 'cma-dark'],
    ['chronobot-light', 'cma-light'],
  ])('migrates the historical product id %s to %s', (historical, canonical) => {
    window.localStorage.setItem(LEGACY_STORAGE_KEY, historical);

    expect(storedThemeId()).toBe(canonical);
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe(canonical);
    expect(resolveTheme(canonical.endsWith('dark') ? 'dark' : 'light', historical).id).toBe(
      canonical,
    );
  });

  it('normalises a historical product id already stored under the canonical key', () => {
    window.localStorage.setItem(STORAGE_KEY, 'chronobot-dark');

    expect(storedThemeId()).toBe('cma-dark');
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe('cma-dark');
  });

  it('prefers valid canonical data when both keys exist', () => {
    window.localStorage.setItem(STORAGE_KEY, 'nord');
    window.localStorage.setItem(LEGACY_STORAGE_KEY, 'dracula');

    expect(storedThemeId()).toBe('nord');
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe('nord');
  });

  it('falls back to valid historical data when canonical data is malformed', () => {
    window.localStorage.setItem(STORAGE_KEY, 'no-such-theme');
    window.localStorage.setItem(LEGACY_STORAGE_KEY, 'dracula');

    expect(storedThemeId()).toBe('dracula');
    expect(window.localStorage.getItem(STORAGE_KEY)).toBe('dracula');
  });

  it('returns null when neither key contains a known palette', () => {
    window.localStorage.setItem(STORAGE_KEY, 'no-such-theme');
    window.localStorage.setItem(LEGACY_STORAGE_KEY, 'also-unknown');

    expect(storedThemeId()).toBeNull();
  });

  it('uses a historical palette even when its migration write is blocked', () => {
    window.localStorage.setItem(LEGACY_STORAGE_KEY, 'dracula');
    vi.spyOn(window.localStorage, 'setItem').mockImplementation(() => {
      throw new DOMException('quota', 'QuotaExceededError');
    });

    expect(storedThemeId()).toBe('dracula');
  });

  it('returns null instead of throwing when reads are blocked', () => {
    vi.spyOn(window.localStorage, 'getItem').mockImplementation(() => {
      throw new DOMException('denied', 'SecurityError');
    });

    expect(storedThemeId()).toBeNull();
  });
});
