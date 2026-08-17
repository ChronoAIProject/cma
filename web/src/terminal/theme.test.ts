import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { activeThemeName, terminalTheme } from './theme';

/**
 * xterm renders to canvas and cannot resolve CSS custom properties, so the
 * terminal palette is duplicated as literals in theme.ts. Duplication that
 * nothing checks is duplication that drifts, so this reads tokens.css and
 * fails if the two ever disagree.
 */

// Read from disk rather than importing: vitest does not run Vite's CSS
// pipeline, so `?raw` yields nothing here. vitest's cwd is web/.
const css = readFileSync(resolve(process.cwd(), 'src/styles/tokens.css'), 'utf8');

/** Read a token from a specific block of tokens.css. */
function token(name: string, block: 'light' | 'dark'): string {
  const source =
    block === 'dark'
      ? (/:root\[data-theme='dark'\]\s*\{([^}]*)\}/.exec(css)?.[1] ?? '')
      : (/^:root\s*\{([^}]*)\}/m.exec(css)?.[1] ?? '');

  const match = new RegExp(`--${name}:\\s*([^;]+);`).exec(source);
  if (match?.[1] === undefined) {
    throw new Error(`token --${name} not found in the ${block} block`);
  }
  return match[1].trim().toUpperCase();
}

describe('terminal theme', () => {
  it.each(['light', 'dark'] as const)('matches the %s design tokens', (name) => {
    const theme = terminalTheme(name);

    expect(theme.background?.toUpperCase()).toBe(token('surface', name));
    expect(theme.foreground?.toUpperCase()).toBe(token('ink', name));
    expect(theme.cursor?.toUpperCase()).toBe(token('accent', name));
    // The slots that carry product meaning are the semantic tokens themselves.
    expect(theme.green?.toUpperCase()).toBe(token('live', name));
    expect(theme.yellow?.toUpperCase()).toBe(token('wait', name));
    expect(theme.red?.toUpperCase()).toBe(token('fail', name));
    expect(theme.brightBlack?.toUpperCase()).toBe(token('ended', name));
    expect(theme.brightBlack?.toUpperCase()).toBe(token('ink-3', name));
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

  it('does not use the dark accent in the light theme', () => {
    // The lime accent fill measures 1.35:1 on white, far below AA, so the
    // light palette takes the accent INK rather than the shared fill.
    const light = JSON.stringify(terminalTheme('light')).toUpperCase();
    expect(light).not.toContain('#C6F24E');
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
