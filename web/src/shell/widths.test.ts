import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { canvasFits, railIsColumn, widthFor } from './widths';

/**
 * §16 Responsive — docs/Web_Style.md.
 *
 * "The number of columns changes; the bands never do. 52px header and 40px
 * footer hold at every width, so the page reads as the same shape on a phone
 * as on a desk."
 */

function tokens(): string {
  return readFileSync(resolve(process.cwd(), 'src/styles/tokens.css'), 'utf8');
}

/** The block a media query opens, so its overrides can be read on their own. */
function mediaBlock(css: string, query: string): string {
  const start = css.indexOf(`@media ${query}`);
  expect(start, query).toBeGreaterThan(-1);
  return css.slice(start, css.indexOf('\n}', css.indexOf('{', start)));
}

describe('§16 four widths', () => {
  it.each([
    [1440, 'wide'],
    [1201, 'wide'],
    [1200, 'wide'],
    [1199, 'medium'],
    [960, 'medium'],
    [959, 'narrow'],
    [480, 'narrow'],
    [479, 'compact'],
    [390, 'compact'],
  ] as const)('reads %ipx as %s', (px, expected) => {
    expect(widthFor(px)).toBe(expected);
  });

  it('keeps the rail a column down to 60rem and no further', () => {
    // ≥75rem: three columns, nothing collapses on its own. 60–75rem: the rail
    // arrives collapsed to its 48px strip — still a column. Below 60rem it
    // leaves the layout, so the terminal keeps the full width.
    expect(railIsColumn('wide')).toBe(true);
    expect(railIsColumn('medium')).toBe(true);
    expect(railIsColumn('narrow')).toBe(false);
    expect(railIsColumn('compact')).toBe(false);
  });

  it('offers the canvas at 60rem and above only', () => {
    // "A grid of terminals in a narrow window is several terminals nobody can
    // read." Below that the toggle is disabled with its reason, not removed.
    expect(canvasFits('wide')).toBe(true);
    expect(canvasFits('medium')).toBe(true);
    expect(canvasFits('narrow')).toBe(false);
    expect(canvasFits('compact')).toBe(false);
  });
});

describe('§16 at 390px', () => {
  const compact = () => mediaBlock(tokens(), '(max-width: 29.999rem)');

  it('grows every target to 44px', () => {
    // §7 and §10 put the minimum target at 44px below 30rem. Raising --hit
    // alone would satisfy the token and none of the buttons, because a control
    // takes its height from --control-h.
    const block = compact();
    for (const token of [
      '--hit',
      '--control-h',
      '--control-h-sm',
      '--control-h-md',
      '--control-h-lg',
    ]) {
      expect(block, token).toContain(`${token}: 44px`);
    }
  });

  it('drops the app bar to 52px', () => {
    expect(compact()).toContain('--bar-height: 52px');
  });

  it('leaves the bands exactly where they were', () => {
    // The one rule §16 opens with. A band that changed height with the window
    // would make the page a different shape on a phone, which is what the band
    // grid exists to prevent.
    const block = compact();
    expect(block).not.toContain('--band-head');
    expect(block).not.toContain('--band-foot');

    const css = tokens();
    expect(css).toContain('--band-head: 56px');
    expect(css).toContain('--band-foot: 40px');
    // And no media query anywhere redefines them.
    expect(css.match(/--band-(head|foot):/g)).toHaveLength(2);
  });
});
