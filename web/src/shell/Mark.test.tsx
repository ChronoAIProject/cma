import { readFileSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { Mark, gapFor } from './Mark';

/**
 * The mark and the faces beneath it — docs/Web_Style.md §17 and §4.
 *
 * The mark was once a glyph in a pixel face §4 has since retired. A mark that
 * depends on a typeface goes dark when that typeface does, which is what
 * these assertions exist to prevent recurring.
 */

const FONTS = resolve(process.cwd(), 'src/assets/fonts');

describe('the mark', () => {
  it('is four squares and no glyph', () => {
    const { container } = render(<Mark />);
    const mark = container.querySelector('.mark');
    expect(mark?.querySelectorAll('.mark__cell')).toHaveLength(4);
    // No text content at all: nothing here can be affected by a missing face,
    // and nothing is announced, because the wordmark beside it is the name.
    expect(mark?.textContent).toBe('');
    expect(mark).toHaveAttribute('aria-hidden', 'true');
  });

  it('lights exactly one cell', () => {
    const { container } = render(<Mark />);
    expect(container.querySelectorAll('.mark__cell--lit')).toHaveLength(1);
  });

  it('stays legible at a 16px favicon', () => {
    // §17: the gap scales with the mark so the cells never fuse. At 16px a
    // second pixel of gap would cost a quarter of each cell.
    const { container } = render(<Mark size={16} />);
    const mark = container.querySelector('.mark') as HTMLElement;
    expect(mark.style.width).toBe('16px');
    expect(mark.style.gap).toBe('1px');
  });

  it.each([
    [96, 8],
    [128, 8],
    [40, 4],
    [64, 4],
    [18, 2],
    [24, 2],
    [16, 1],
    [12, 1],
  ])('gives a %ipx mark a %ipx gap', (size, gap) => {
    expect(gapFor(size)).toBe(gap);
  });
});

describe('the platform faces', () => {
  it('vendors both faces with their licences, and nothing else', () => {
    // Exact equality, deliberately: §4 retires a face by removing it, and a
    // retired face lingering beside the vendored pair would ship dead bytes.
    expect(readdirSync(FONTS).sort()).toEqual([
      'Figtree-Variable.woff2',
      'JetBrainsMono-Variable.woff2',
      'OFL-Figtree.txt',
      'OFL-JetBrainsMono.txt',
    ]);
  });

  it('requests no font from a third party', () => {
    // §4: a font request to someone else's host is a request that can be
    // logged, blocked, or slow.
    const css = readFileSync(resolve(process.cwd(), 'src/styles/base.css'), 'utf8');
    for (const match of css.matchAll(/url\((['"])(.*?)\1\)/g)) {
      expect(match[2]).toMatch(/^\.\.\/assets\/fonts\//);
    }
    expect(css).not.toMatch(/@import/);
  });

  it('swaps rather than blocking, and names a system fallback', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/styles/base.css'), 'utf8');
    const faces = [...css.matchAll(/@font-face\s*\{([^}]*)\}/g)].map((match) => match[1] ?? '');
    expect(faces).toHaveLength(2);
    for (const face of faces) {
      expect(face).toContain('font-display: swap');
    }

    // Text must be readable before the faces arrive, so each stack carries a
    // system fallback after the vendored name.
    const tokens = readFileSync(resolve(process.cwd(), 'src/styles/tokens.css'), 'utf8');
    const stacks: Record<string, string> = {
      '--font-ui': "'Figtree'",
      '--font-mono': "'JetBrains Mono'",
    };
    for (const [name, face] of Object.entries(stacks)) {
      const stack = new RegExp(`${name}:\\s*([^;]+);`).exec(tokens)?.[1] ?? '';
      // The vendored face leads and a system fallback follows it.
      expect(stack.trim().startsWith(face)).toBe(true);
      expect(stack.split(',').length).toBeGreaterThan(1);
    }
  });

  it('licenses both faces under the OFL', () => {
    const figtree = readFileSync(join(FONTS, 'OFL-Figtree.txt'), 'utf8');
    expect(figtree).toContain('SIL OPEN FONT LICENSE Version 1.1');
    expect(figtree).toContain('Figtree Project Authors');

    const mono = readFileSync(join(FONTS, 'OFL-JetBrainsMono.txt'), 'utf8');
    expect(mono).toContain('SIL OPEN FONT LICENSE Version 1.1');
    expect(mono).toContain('JetBrains Mono Project Authors');
  });
});
