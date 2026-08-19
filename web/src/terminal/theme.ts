import type { ITheme } from '@xterm/xterm';

/**
 * The 16-slot ANSI palette, derived from the design tokens per §2 and
 * authoritative per §9. Do not invent terminal colours here.
 *
 * These are literals rather than CSS custom properties because xterm renders to
 * canvas and cannot resolve `var()`. They are kept in lockstep with
 * src/styles/design-platform.css by the palette test, which resolves each
 * alias through to its static value and fails if the two diverge.
 *
 * The product has one accent, so cyan and magenta are no longer a pair carrying
 * product meaning: they are ANSI slots, chosen for legibility on the surface
 * they sit on. The slots that DO carry product meaning — green, yellow, red,
 * bright black — are the semantic tokens, and the test pins them.
 *
 * The greyscale ends invert between the two themes rather than being shared.
 * A terminal's black slot means "the ink end of the ramp", not the colour
 * black: rendered literally on a white ground it writes paper onto paper, and
 * the text is gone.
 */

interface Palette {
  readonly background: string;
  readonly foreground: string;
  readonly cursor: string;
  readonly selection: string;
  readonly ansi: Omit<ITheme, 'background' | 'foreground' | 'cursor' | 'selectionBackground'>;
}

const DARK: Palette = {
  background: '#232324',
  foreground: '#F9FAFB',
  cursor: '#679EFE',
  // The business primary at 25% alpha, per the terminal component rules.
  selection: 'rgba(103, 158, 254, 0.25)',
  ansi: {
    // One step below the ground, so the ink end still reads as behind it.
    black: '#151517',
    red: '#F25A5A',
    green: '#22C55E',
    yellow: '#F59E0B',
    blue: '#679EFE',
    magenta: '#C08BE0',
    cyan: '#5FD3C4',
    white: '#CFD3D6',
    brightBlack: '#ADB2B8',
    brightRed: '#FF8A8A',
    brightGreen: '#4ADE80',
    brightYellow: '#FBBF24',
    brightBlue: '#93B8FE',
    brightMagenta: '#D7B0EE',
    brightCyan: '#93E6DB',
    brightWhite: '#F9FAFB',
  },
};

const LIGHT: Palette = {
  background: '#FFFFFF',
  foreground: '#0F1115',
  cursor: '#4176E6',
  selection: 'rgba(65, 118, 230, 0.25)',
  ansi: {
    black: '#0F1115',
    red: '#EC1313',
    green: '#22C55E',
    yellow: '#F59E0B',
    blue: '#4176E6',
    magenta: '#8B3FBF',
    cyan: '#0E7C74',
    white: '#61666B',
    brightBlack: '#81858C',
    brightRed: '#B00E0E',
    brightGreen: '#15803D',
    brightYellow: '#B45309',
    brightBlue: '#2F58B4',
    brightMagenta: '#6B2E94',
    brightCyan: '#0A5C56',
    brightWhite: '#0F1115',
  },
};

export type ThemeName = 'light' | 'dark';

export function terminalTheme(name: ThemeName): ITheme {
  const palette = name === 'dark' ? DARK : LIGHT;
  return {
    background: palette.background,
    foreground: palette.foreground,
    cursor: palette.cursor,
    cursorAccent: palette.background,
    selectionBackground: palette.selection,
    ...palette.ansi,
  };
}

/**
 * Resolve the theme the same way the palette does — by reading the mark the
 * palette itself selects on.
 *
 * That mark is already resolved: whoever set it turned "system" into an answer
 * first. Asking the media query here as well would give a different answer to
 * the same question the moment somebody chose light on a dark machine, and the
 * terminal is the one surface where that would be visible as a mismatch rather
 * than as a theme.
 */
export function activeThemeName(): ThemeName {
  return document.body.hasAttribute('data-ds-dark-theme') ? 'dark' : 'light';
}
