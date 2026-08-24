/**
 * Selectable palettes for the terminal canvas.
 *
 * Transcribed from docs/Web_Style.md section 10, where they are authoritative,
 * and vendored as data rather than taken as a dependency: a theme is twenty hex
 * values against a fixed interface, and a package for that would add
 * supply-chain risk with nothing to show for it.
 *
 * Scope is the terminal canvas and nothing else. The surrounding interface
 * stays on the tokens in section 2, because a page whose chrome shifts with a
 * terminal preference stops being one product.
 */

import type { ITheme } from '@xterm/xterm';
import { terminalTheme, type ThemeName } from './theme';

/** A named palette, and the appearance it belongs beside. */
export interface TerminalTheme {
  readonly id: string;
  readonly label: string;
  /**
   * Which appearance this palette suits.
   *
   * Load-bearing: a dark palette chosen in light appearance puts a near-black
   * rectangle in the middle of a light page, so the selector only offers
   * themes matching the current appearance.
   */
  readonly appearance: ThemeName;
  readonly theme: ITheme;
}

/** The product's own palette, resolved from section 6. */
function own(appearance: ThemeName): TerminalTheme {
  return {
    id: `cma-${appearance}`,
    label: 'CMA',
    appearance,
    theme: terminalTheme(appearance),
  };
}

const DRACULA: ITheme = {
  background: '#282A36',
  foreground: '#F8F8F2',
  cursor: '#F8F8F2',
  cursorAccent: '#282A36',
  selectionBackground: 'rgba(68, 71, 90, 0.75)',
  black: '#21222C',
  red: '#FF5555',
  green: '#50FA7B',
  yellow: '#F1FA8C',
  blue: '#BD93F9',
  magenta: '#FF79C6',
  cyan: '#8BE9FD',
  white: '#F8F8F2',
  brightBlack: '#6272A4',
  brightRed: '#FF6E6E',
  brightGreen: '#69FF94',
  brightYellow: '#FFFFA5',
  brightBlue: '#D6ACFF',
  brightMagenta: '#FF92DF',
  brightCyan: '#A4FFFF',
  brightWhite: '#FFFFFF',
};

const NORD: ITheme = {
  background: '#2E3440',
  foreground: '#D8DEE9',
  cursor: '#D8DEE9',
  cursorAccent: '#2E3440',
  selectionBackground: 'rgba(67, 76, 94, 0.8)',
  black: '#3B4252',
  red: '#BF616A',
  green: '#A3BE8C',
  yellow: '#EBCB8B',
  blue: '#81A1C1',
  magenta: '#B48EAD',
  cyan: '#88C0D0',
  white: '#E5E9F0',
  brightBlack: '#4C566A',
  brightRed: '#BF616A',
  brightGreen: '#A3BE8C',
  brightYellow: '#EBCB8B',
  brightBlue: '#81A1C1',
  brightMagenta: '#B48EAD',
  brightCyan: '#8FBCBB',
  brightWhite: '#ECEFF4',
};

const SOLARIZED_DARK: ITheme = {
  background: '#002B36',
  foreground: '#839496',
  cursor: '#93A1A1',
  cursorAccent: '#002B36',
  selectionBackground: 'rgba(7, 54, 66, 0.9)',
  black: '#073642',
  red: '#DC322F',
  green: '#859900',
  yellow: '#B58900',
  blue: '#268BD2',
  magenta: '#D33682',
  cyan: '#2AA198',
  white: '#EEE8D5',
  brightBlack: '#586E75',
  brightRed: '#CB4B16',
  brightGreen: '#586E75',
  brightYellow: '#657B83',
  brightBlue: '#839496',
  brightMagenta: '#6C71C4',
  brightCyan: '#93A1A1',
  brightWhite: '#FDF6E3',
};

const SOLARIZED_LIGHT: ITheme = {
  background: '#FDF6E3',
  foreground: '#657B83',
  cursor: '#586E75',
  cursorAccent: '#FDF6E3',
  selectionBackground: 'rgba(238, 232, 213, 0.9)',
  black: '#073642',
  red: '#DC322F',
  green: '#859900',
  yellow: '#B58900',
  blue: '#268BD2',
  magenta: '#D33682',
  cyan: '#2AA198',
  white: '#EEE8D5',
  brightBlack: '#002B36',
  brightRed: '#CB4B16',
  brightGreen: '#586E75',
  brightYellow: '#657B83',
  brightBlue: '#839496',
  brightMagenta: '#6C71C4',
  brightCyan: '#93A1A1',
  brightWhite: '#FDF6E3',
};

const GITHUB_LIGHT: ITheme = {
  background: '#FFFFFF',
  foreground: '#24292E',
  cursor: '#24292E',
  cursorAccent: '#FFFFFF',
  selectionBackground: 'rgba(3, 102, 214, 0.2)',
  black: '#24292E',
  red: '#D73A49',
  green: '#22863A',
  yellow: '#B08800',
  blue: '#0366D6',
  magenta: '#6F42C1',
  cyan: '#1B7C83',
  white: '#6A737D',
  brightBlack: '#959DA5',
  brightRed: '#CB2431',
  brightGreen: '#28A745',
  brightYellow: '#DBAB09',
  brightBlue: '#2188FF',
  brightMagenta: '#8A63D2',
  brightCyan: '#3192AA',
  brightWhite: '#D1D5DA',
};

/** Every palette on offer, in the order the selector lists them. */
export function allThemes(): readonly TerminalTheme[] {
  return [
    own('dark'),
    own('light'),
    { id: 'dracula', label: 'Dracula', appearance: 'dark', theme: DRACULA },
    { id: 'nord', label: 'Nord', appearance: 'dark', theme: NORD },
    {
      id: 'solarized-dark',
      label: 'Solarized Dark',
      appearance: 'dark',
      theme: SOLARIZED_DARK,
    },
    {
      id: 'solarized-light',
      label: 'Solarized Light',
      appearance: 'light',
      theme: SOLARIZED_LIGHT,
    },
    { id: 'github-light', label: 'GitHub Light', appearance: 'light', theme: GITHUB_LIGHT },
  ];
}

/** The palettes that suit an appearance. */
export function themesFor(appearance: ThemeName): readonly TerminalTheme[] {
  return allThemes().filter((t) => t.appearance === appearance);
}

const STORAGE_KEY = 'cma:terminal-theme';
const LEGACY_STORAGE_KEY = 'chrono-bot:terminal-theme';

function canonicalThemeId(value: string | null): string | null {
  if (value === 'chronobot-dark') {
    return 'cma-dark';
  }
  if (value === 'chronobot-light') {
    return 'cma-light';
  }
  return value !== null && allThemes().some((theme) => theme.id === value) ? value : null;
}

function readThemeId(key: string): string | null {
  try {
    return window.localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writeThemeId(id: string): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, id);
  } catch {
    // Applying the palette does not depend on remembering it.
  }
}

/** The palette this browser last chose, if it still exists. */
export function storedThemeId(): string | null {
  const rawCanonical = readThemeId(STORAGE_KEY);
  const canonical = canonicalThemeId(rawCanonical);
  if (canonical !== null) {
    if (canonical !== rawCanonical) {
      writeThemeId(canonical);
    }
    return canonical;
  }

  const historical = canonicalThemeId(readThemeId(LEGACY_STORAGE_KEY));
  if (historical === null) {
    return null;
  }

  writeThemeId(historical);
  return historical;
}

/** Remember a palette choice. */
export function storeThemeId(id: string): void {
  writeThemeId(canonicalThemeId(id) ?? id);
}

/**
 * The palette to paint with, given the appearance and any stored choice.
 *
 * A stored choice that does not suit the current appearance is ignored rather
 * than honoured, so switching the page to light cannot leave a dark rectangle
 * in the middle of it. The choice is kept, not cleared — switching back
 * restores it.
 */
export function resolveTheme(appearance: ThemeName, storedId: string | null): TerminalTheme {
  const candidates = themesFor(appearance);
  const canonicalId = canonicalThemeId(storedId);
  const chosen = candidates.find((t) => t.id === canonicalId);
  // The product's own palette is always first for an appearance, so it is the
  // fallback by construction rather than by a second lookup.
  return chosen ?? candidates[0] ?? own(appearance);
}
