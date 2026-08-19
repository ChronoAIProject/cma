import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
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

  it('reads the same mark the palette selects on', () => {
    // Not the media query. Somebody who chose light on a dark machine has a
    // light page, and a terminal that consulted the machine directly would be
    // the one dark rectangle on it.
    document.body.toggleAttribute('data-ds-dark-theme', true);
    expect(activeThemeName()).toBe('dark');

    document.body.toggleAttribute('data-ds-dark-theme', false);
    expect(activeThemeName()).toBe('light');
  });
});

describe('the terminal card', () => {
  const css = readFileSync(resolve(process.cwd(), 'src/terminal/TerminalSurface.css'), 'utf8');
  const tsx = readFileSync(resolve(process.cwd(), 'src/terminal/TerminalSurface.tsx'), 'utf8');

  it('takes the same card a code block takes', () => {
    /*
     * It is the same kind of content — a program's own output in its own
     * font — and two treatments for that would make the two planes read as
     * two products.
     */
    const rule = /\.terminal\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    expect(rule).toContain('background: var(--dsw-alias-markdown-code-block)');
    expect(rule).toContain('border-radius: 12px');
  });

  it('takes the same code stack as everything else', () => {
    // A command and its output should line up whether they are read here or
    // in a transcript.
    for (const face of ["'SF Mono'", "'JetBrains Mono'", "'Fira Code'"]) {
      expect(tsx).toContain(face);
    }
  });

  it('keeps a bare monospace tail, which nothing else may', () => {
    /*
     * The one permitted exception. Elsewhere `monospace` resolves to a CJK
     * serif on Windows and wrecks the column widths; here, a terminal with no
     * font at all is not a terminal, so a last resort has to exist.
     */
    expect(tsx).toMatch(/Menlo, monospace/);
  });

  it('sets a line height in pixels rather than a ratio somebody guessed', () => {
    // 22px at 13px, matching the code blocks. Written as the division so the
    // two numbers stay legible and stay together.
    expect(tsx).toContain('fontSize: 13');
    expect(tsx).toContain('lineHeight: 22 / 13');
  });
});
