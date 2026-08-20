import { describe, expect, it } from 'vitest';
import { allThemes, resolveTheme, themesFor } from './themes';

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
