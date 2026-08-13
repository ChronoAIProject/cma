import type { ITheme } from '@xterm/xterm';

/**
 * The 16-slot ANSI palette, transcribed from docs/Web_Style.md section 6,
 * where it is authoritative. Do not invent terminal colours here.
 *
 * These are literals rather than CSS custom properties because xterm renders
 * to canvas and cannot resolve `var()`. They are kept in lockstep with
 * src/styles/tokens.css by the palette test, which fails if the two diverge.
 */

interface Palette {
  readonly background: string;
  readonly foreground: string;
  readonly cursor: string;
  readonly selection: string;
  readonly ansi: Omit<ITheme, 'background' | 'foreground' | 'cursor' | 'selectionBackground'>;
}

const DARK: Palette = {
  background: '#0B0F17',
  foreground: '#E8EEF5',
  cursor: '#00E5FF',
  // --accent at 25% alpha, per the terminal component rules.
  selection: 'rgba(0, 229, 255, 0.25)',
  ansi: {
    black: '#0B0F17',
    red: '#FF3B4E',
    green: '#00FFA3',
    yellow: '#FFB800',
    blue: '#00A8FF',
    magenta: '#FF2E97',
    cyan: '#00E5FF',
    white: '#E8EEF5',
    brightBlack: '#7C8A9E',
    brightRed: '#FF6B78',
    brightGreen: '#6BFFC4',
    brightYellow: '#FFD166',
    brightBlue: '#5CC6FF',
    brightMagenta: '#FF6BB8',
    brightCyan: '#6BF0FF',
    brightWhite: '#FFFFFF',
  },
};

const LIGHT: Palette = {
  background: '#FFFFFF',
  foreground: '#3D4757',
  cursor: '#006D8F',
  selection: 'rgba(0, 109, 143, 0.25)',
  ansi: {
    black: '#E9EDF3',
    red: '#C0182B',
    green: '#00674A',
    yellow: '#7A5200',
    blue: '#00557A',
    magenta: '#B3006B',
    cyan: '#006D8F',
    white: '#3D4757',
    brightBlack: '#5B6779',
    brightRed: '#8E0F1F',
    brightGreen: '#004D37',
    brightYellow: '#5C3E00',
    brightBlue: '#003F5C',
    brightMagenta: '#8A0052',
    brightCyan: '#00536D',
    brightWhite: '#0A0E14',
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
