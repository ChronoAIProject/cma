import type { ITheme } from '@xterm/xterm';

/**
 * The 16-slot ANSI palette, derived from the design tokens per §2 and
 * authoritative per Section 14. Do not invent terminal colours here.
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
  background: '#0C0D0A',
  foreground: '#EDF2E2',
  cursor: '#C3FD43',
  // The business primary at 25% alpha, per the terminal component rules.
  selection: 'rgba(195, 253, 67, 0.25)',
  ansi: {
    // One step below the ground, so the ink end still reads as behind it.
    black: '#060606',
    red: '#FF5A45',
    green: '#C3FD43',
    yellow: '#FFB020',
    blue: '#72B7FF',
    magenta: '#E69AF2',
    cyan: '#69D7C6',
    white: '#A9B199',
    brightBlack: '#6B7361',
    brightRed: '#FF806F',
    brightGreen: '#D7FF7D',
    brightYellow: '#FFC45A',
    brightBlue: '#9ACBFF',
    brightMagenta: '#F0BCF7',
    brightCyan: '#9AE8DC',
    brightWhite: '#EDF2E2',
  },
};

const LIGHT: Palette = {
  background: '#FFFBF2',
  foreground: '#15283C',
  cursor: '#0E61AC',
  selection: 'rgba(14, 97, 172, 0.25)',
  ansi: {
    black: '#15283C',
    red: '#BC3B26',
    green: '#3E7A22',
    yellow: '#A9720B',
    blue: '#0E61AC',
    magenta: '#7B3F93',
    cyan: '#0E766E',
    white: '#4C5A68',
    brightBlack: '#8C8474',
    brightRed: '#982D1C',
    brightGreen: '#2D6017',
    brightYellow: '#875A08',
    brightBlue: '#073A69',
    brightMagenta: '#603074',
    brightCyan: '#0A5B55',
    brightWhite: '#15283C',
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
