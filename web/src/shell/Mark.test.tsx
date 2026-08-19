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
  it('vendors the one face it downloads, with its licence and nothing else', () => {
    // Exact equality, deliberately: §4 retires a face by removing it, and a
    // retired face lingering here would ship dead bytes. §20.3 retired the
    // interface face for the system stack, so only the mono face is vendored
    // — code and terminal output cannot be left to whatever a platform calls
    // monospace, and everything else can.
    expect(readdirSync(FONTS).sort()).toEqual([
      'JetBrainsMono-Variable.woff2',
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
    expect(faces).toHaveLength(1);
    for (const face of faces) {
      expect(face).toContain('font-display: swap');
    }

    const tokens = readFileSync(resolve(process.cwd(), 'src/styles/tokens.css'), 'utf8');
    const stackOf = (name: string) =>
      (new RegExp(`${name}:\\s*([^;]+);`).exec(tokens)?.[1] ?? '').trim();

    // The mono face is downloaded, so it leads its stack and a system fallback
    // follows: text must be readable before it arrives.
    const mono = stackOf('--font-mono');
    expect(mono.startsWith("'JetBrains Mono'")).toBe(true);
    expect(mono.split(',').length).toBeGreaterThan(1);

    // The interface stack downloads nothing — §20.3. It must therefore name no
    // face this repository would have to ship, and start with a platform's
    // own, which is what makes it correct on the first paint rather than the
    // second.
    const ui = stackOf('--font-ui');
    expect(ui).toMatch(/^-apple-system/);
    expect(ui).not.toContain('JetBrains Mono');
    for (const face of readdirSync(FONTS).filter((f) => f.endsWith('.woff2'))) {
      const name = face.replace(/-Variable\.woff2$/, '');
      expect(ui).not.toContain(name);
    }
  });

  it('licenses the face it ships under the OFL', () => {
    // A vendored face travels with its licence or it should not travel.
    const mono = readFileSync(join(FONTS, 'OFL-JetBrainsMono.txt'), 'utf8');
    expect(mono).toContain('SIL OPEN FONT LICENSE Version 1.1');
    expect(mono).toContain('JetBrains Mono Project Authors');
  });
});
