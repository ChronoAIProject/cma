import { readFileSync, readdirSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

/**
 * The faces this product ships — `docs/Web_Style.md` §4 and §20.3.
 *
 * These rules are about the product's typography and about nothing else, but
 * they lived inside `shell/Mark.test.tsx` because the mark used to be a glyph
 * in a face §4 has since retired — a mark that depends on a typeface goes dark
 * when the typeface does.
 *
 * The mark stopped being a glyph, then stopped existing: an asset replaced it
 * and the component was deleted. Its test file went with it, correctly, and
 * took this suite along as a bystander — the only thing enforcing §4's font
 * rules disappeared in a change about branding, and every gate stayed green
 * because deleting a test never fails one.
 *
 * Restored here, under the name of what it actually checks, so the next
 * component to be retired cannot take it again.
 */

const FONTS = resolve(process.cwd(), 'src/assets/fonts');
const STYLES = resolve(process.cwd(), 'src/styles');

/** Every stylesheet, so a face moving between sheets cannot escape these. */
function styleSheets(): string[] {
  return readdirSync(STYLES, { recursive: true, encoding: 'utf8' })
    .filter((entry) => entry.endsWith('.css'))
    .map((entry) => readFileSync(join(STYLES, entry), 'utf8'));
}

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
    // logged, blocked, or slow. Checked across every sheet, because the rule
    // is about the product and not about whichever file the face sits in.
    for (const css of styleSheets()) {
      for (const match of css.matchAll(/url\((['"])(.*?)\1\)/g)) {
        expect(match[2]).toMatch(/^(\.\.\/assets\/fonts\/|data:)/);
      }
      expect(css).not.toMatch(/@import/);
    }
  });

  it('swaps rather than blocking, and names a system fallback', () => {
    const faces = styleSheets().flatMap((css) =>
      [...css.matchAll(/@font-face\s*\{([^}]*)\}/g)].map((match) => match[1] ?? ''),
    );
    expect(faces).toHaveLength(1);
    for (const face of faces) {
      expect(face).toContain('font-display: swap');
    }

    const tokens = readFileSync(join(STYLES, 'tokens.css'), 'utf8');
    const stackOf = (name: string) =>
      (new RegExp(`${name}:\\s*([^;]+);`).exec(tokens)?.[1] ?? '').replace(/\s+/g, ' ').trim();

    /*
     * The vendored face is named in the code stack but does not lead it: a
     * machine that already has SF Mono renders in it and never fetches ours,
     * which is a download avoided rather than a fallback. What matters is
     * that the face is reachable at all, and that every name before it is one
     * a platform ships.
     */
    /*
     * The family and the file come from the declaration itself rather than
     * from the filename: what a stack has to name is what `@font-face` calls
     * the face, and the two agreeing is exactly what would otherwise drift.
     */
    const declared = /font-family:\s*'([^']+)'/.exec(faces[0] ?? '')?.[1] ?? '';
    const file = /url\('\.\.\/assets\/fonts\/([^']+)'\)/.exec(faces[0] ?? '')?.[1] ?? '';
    expect(declared).not.toBe('');
    expect(readdirSync(FONTS)).toContain(file);

    const mono = stackOf('--ds-font-family-code');
    expect(mono).toContain(`'${declared}'`);
    expect(mono.split(',').length).toBeGreaterThan(1);

    /*
     * No bare `monospace` tail. On Windows that keyword resolves to a CJK
     * serif, which is the one thing a terminal cannot use — column widths are
     * the entire reason this stack exists.
     */
    expect(mono.split(',').map((face) => face.trim())).not.toContain('monospace');

    // The interface stack downloads nothing — §20.3. It must therefore name no
    // face this repository would have to ship, and start with a platform's
    // own, which is what makes it correct on the first paint rather than the
    // second.
    const ui = stackOf('--dsw-font-family');
    expect(ui).toMatch(/^-apple-system/);
    expect(ui).not.toContain(declared);
  });

  it('licenses the face it ships under the OFL', () => {
    // A vendored face travels with its licence or it should not travel.
    const mono = readFileSync(join(FONTS, 'OFL-JetBrainsMono.txt'), 'utf8');
    expect(mono).toContain('SIL OPEN FONT LICENSE Version 1.1');
    expect(mono).toContain('JetBrains Mono Project Authors');
  });
});
