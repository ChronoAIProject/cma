import { describe, expect, it } from 'vitest';
import { asHex, resolveToken, themes } from '../test/palette';
import { activeThemeName, terminalTheme } from './theme';

/**
 * xterm renders to canvas and cannot resolve CSS custom properties, so the
 * terminal palette is duplicated as literals in theme.ts. Duplication that
 * nothing checks is duplication that drifts, so this resolves the design
 * tokens itself and fails if the two ever disagree.
 */

const PALETTE = themes();

/** A design alias, resolved through its tiers, in the form xterm speaks. */
function token(name: string, block: 'light' | 'dark'): string {
  return asHex(resolveToken(`dsw-alias-${name}`, PALETTE[block])).toUpperCase();
}

describe('terminal theme', () => {
  it.each(['light', 'dark'] as const)('matches the %s design tokens', (name) => {
    const theme = terminalTheme(name);

    expect(theme.background?.toUpperCase()).toBe(token('bg-layer-1', name));
    expect(theme.foreground?.toUpperCase()).toBe(token('label-primary', name));
    expect(theme.cursor?.toUpperCase()).toBe(token('state-business-primary', name));
    // The slots that carry product meaning are the semantic tokens themselves.
    expect(theme.green?.toUpperCase()).toBe(token('state-success-primary', name));
    expect(theme.yellow?.toUpperCase()).toBe(token('state-warn-primary', name));
    expect(theme.red?.toUpperCase()).toBe(token('state-error-primary', name));
    expect(theme.brightBlack?.toUpperCase()).toBe(token('label-tertiary', name));
    expect(theme.white?.toUpperCase()).toBe(token('label-secondary', name));
  });

  it.each(['light', 'dark'] as const)('defines all sixteen ANSI slots in %s', (name) => {
    const theme = terminalTheme(name);
    const slots = [
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

    for (const slot of slots) {
      expect(theme[slot], `${name}.${slot}`).toMatch(/^#[0-9a-fA-F]{6}$/);
    }
  });

  it('keeps each theme greyscale end off its own ground', () => {
    /*
     * The one failure this palette has actually had: a slot carried over from
     * the other theme. A black slot holding the light theme's paper writes
     * white on white, and the text is simply gone — so each theme's ink end
     * must sit against its own ground, not the other's.
     */
    for (const name of ['light', 'dark'] as const) {
      const theme = terminalTheme(name);
      const ground = token('bg-layer-1', name);
      for (const slot of ['black', 'white', 'brightBlack', 'brightWhite'] as const) {
        expect(theme[slot]?.toUpperCase(), `${name}.${slot}`).not.toBe(ground);
      }
    }

    // And the accent is per-theme: the light one is darker for contrast on
    // white, so finding the dark one here means a slot was shared.
    expect(JSON.stringify(terminalTheme('light')).toUpperCase()).not.toContain('#679EFE');
  });

  it('honours an explicit theme choice over the system preference', () => {
    document.documentElement.setAttribute('data-theme', 'light');
    expect(activeThemeName()).toBe('light');

    document.documentElement.setAttribute('data-theme', 'dark');
    expect(activeThemeName()).toBe('dark');

    document.documentElement.removeAttribute('data-theme');
    expect(['light', 'dark']).toContain(activeThemeName());
  });
});
