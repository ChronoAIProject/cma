import type { ITheme } from '@xterm/xterm';

/**
 * The 16-slot ANSI palette, derived from the tokens in docs/Web_Style.md §2 and
 * authoritative per §9. Do not invent terminal colours here.
 *
 * These are literals rather than CSS custom properties because xterm renders to
 * canvas and cannot resolve `var()`. They are kept in lockstep with
 * src/styles/tokens.css by the palette test, which fails if the two diverge.
 *
 * The product has one accent, so cyan and magenta are no longer a pair carrying
 * product meaning: they are ANSI slots, chosen for legibility on the surface
 * they sit on. The slots that DO carry product meaning — green, yellow, red,
 * bright black — are the semantic tokens, and the test pins them.
 */

interface Palette {
  readonly background: string;
  readonly foreground: string;
  readonly cursor: string;
  readonly selection: string;
  readonly ansi: Omit<ITheme, 'background' | 'foreground' | 'cursor' | 'selectionBackground'>;
}

const DARK: Palette = {
  background: '#15171B',
  foreground: '#F9FAFB',
  cursor: '#679EFE',
  // --accent at 25% alpha, per the terminal component rules.
  selection: 'rgba(103, 158, 254, 0.25)',
  ansi: {
    black: '#131512',
    red: '#F25A5A',
    green: '#22C55E',
    yellow: '#F59E0B',
    blue: '#679EFE',
    magenta: '#C08BE0',
    cyan: '#5FD3C4',
    white: '#F1F2EE',
    brightBlack: '#81858C',
    brightRed: '#FF8A8A',
    brightGreen: '#4ADE80',
    brightYellow: '#FBBF24',
    brightBlue: '#93B8FE',
    brightMagenta: '#D7B0EE',
    brightCyan: '#93E6DB',
    brightWhite: '#FFFFFF',
  },
};

const LIGHT: Palette = {
  background: '#FFFFFF',
  foreground: '#0F1115',
  cursor: '#4176E6',
  selection: 'rgba(65, 118, 230, 0.25)',
  ansi: {
    black: '#F1F3F5',
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
 * Resolve the theme the same way the stylesheet does: an explicit data-theme
 * wins over the system preference in either direction.
 */
export function activeThemeName(): ThemeName {
  const explicit = document.documentElement.getAttribute('data-theme');
  if (explicit === 'dark' || explicit === 'light') {
    return explicit;
  }
  if (typeof window.matchMedia === 'function') {
    return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  }
  return 'light';
}
