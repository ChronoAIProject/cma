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
  background: '#0F0F11',
  foreground: '#F2F2F3',
  cursor: '#C6F24E',
  // --accent at 25% alpha, per the terminal component rules.
  selection: 'rgba(198, 242, 78, 0.25)',
  ansi: {
    black: '#0F0F11',
    red: '#FF6B6B',
    green: '#C6F24E',
    yellow: '#F2B23E',
    blue: '#7AA2F7',
    magenta: '#C08BE0',
    cyan: '#5FD3C4',
    white: '#F2F2F3',
    brightBlack: '#8A8A94',
    brightRed: '#FF9A9A',
    brightGreen: '#DDF98C',
    brightYellow: '#F7CE7C',
    brightBlue: '#A6C1FA',
    brightMagenta: '#D7B0EE',
    brightCyan: '#93E6DB',
    brightWhite: '#FFFFFF',
  },
};

const LIGHT: Palette = {
  background: '#FFFFFF',
  foreground: '#121214',
  cursor: '#46620B',
  selection: 'rgba(70, 98, 11, 0.25)',
  ansi: {
    black: '#ECEAE6',
    red: '#A12626',
    green: '#46620B',
    yellow: '#8A5A00',
    blue: '#2E4E8F',
    magenta: '#743A8F',
    cyan: '#1F5F58',
    white: '#52525B',
    brightBlack: '#686871',
    brightRed: '#7A1B1B',
    brightGreen: '#334707',
    brightYellow: '#623F00',
    brightBlue: '#213866',
    brightMagenta: '#532866',
    brightCyan: '#16443F',
    brightWhite: '#121214',
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
