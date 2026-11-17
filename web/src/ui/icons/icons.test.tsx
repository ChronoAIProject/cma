import { readFileSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import type { ReactElement } from 'react';
import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import * as icons from './index';

/**
 * The icon set's contract — the shared style contracts.
 *
 * Seventy small files whose only shared property is that they behave the same
 * way. That is exactly the shape nobody re-checks by hand after adding the
 * seventy-first, so it is checked here instead: every glyph takes its colour
 * from the control it sits in, sizes from one prop, and is reachable by name.
 */

const DIR = resolve(process.cwd(), 'src/ui/icons');

const glyphFiles = readdirSync(DIR)
  .filter((name) => name.startsWith('Icon') && name.endsWith('.tsx'))
  .map((name) => name.replace(/\.tsx$/, ''));

const exported = Object.entries(icons).filter(([name]) => name.startsWith('Icon'));

describe('the icon set', () => {
  it('has glyphs at all', () => {
    // A guard on the guards: every assertion below iterates, and an empty
    // directory would satisfy all of them by vacuum.
    expect(glyphFiles.length).toBeGreaterThan(60);
  });

  it('exports every glyph in the directory, and nothing that is not there', () => {
    // The barrel is written by hand. A glyph added as a file but not exported
    // is invisible to every caller; one exported after its file was deleted
    // breaks the build only for whoever imports it next.
    expect(exported.map(([name]) => name).sort()).toEqual([...glyphFiles].sort());
  });

  it.each(glyphFiles)('%s takes its colour from its container', (name) => {
    // `fill="currentColor"` is the whole reason no icon takes a colour prop:
    // a glyph in a hovered button changes with the button. A hardcoded fill
    // would look right in exactly the one place it was drawn for.
    /*
     * Mask contents are geometry, not paint: inside a `<mask>`, white means
     * "keep this" and black means "knock this out", and neither is a colour
     * the glyph ever shows. Scanning them would force a real mask to be
     * rewritten into something that does not work.
     */
    const source = readFileSync(join(DIR, `${name}.tsx`), 'utf8').replace(
      /<mask[\s\S]*?<\/mask>/g,
      '',
    );
    const fills = [...source.matchAll(/fill="([^"]*)"/g)].map((match) => match[1]);
    expect(fills.length, `${name} paints nothing`).toBeGreaterThan(0);
    for (const fill of fills) {
      // "none" is the svg root's own default, not a painted fill.
      expect(['currentColor', 'none'], `${name} hardcodes ${fill}`).toContain(fill);
    }
    expect(source, `${name} sets a stroke colour`).not.toMatch(/stroke="(?!currentColor)[^"]+"/);
  });

  it.each(exported)('%s renders at its own size and at one it is given', (name, Glyph) => {
    const Component = Glyph as (props: { size?: number }) => ReactElement;

    const { container: own } = render(<Component />);
    const drawn = own.querySelector('svg');
    expect(drawn, `${name} renders no svg`).not.toBeNull();
    // Its own drawn size, so a caller that says nothing gets the size the
    // glyph was designed at rather than something inherited.
    expect(drawn?.getAttribute('height')).toMatch(/^\d+(\.\d+)?$/);

    const { container: sized } = render(<Component size={32} />);
    expect(sized.querySelector('svg')?.getAttribute('height')).toBe('32');
  });

  it('carries its provenance in every file', () => {
    // The set is vendored under someone else's licence. A file that lost the
    // notice is a file nobody can tell is not ours.
    for (const name of glyphFiles) {
      const source = readFileSync(join(DIR, `${name}.tsx`), 'utf8');
      expect(source, `${name} has no provenance header`).toContain('deepseek-harness');
      expect(source, `${name} does not point at the licence`).toContain(
        'LICENSE-deepseek-harness.txt',
      );
    }
  });
});
