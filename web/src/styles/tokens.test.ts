import { readFileSync, readdirSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { resolveToken, themes } from '../test/palette';

/**
 * The token layer, checked against docs/Web_Style.md.
 *
 * These are the three properties §137 of the Ground control milestone exists to
 * establish, and each one has failed silently before: `--scrim` was referenced
 * by the shared dialog and defined nowhere, so the dialog rendered with no
 * scrim at all, and the rail's status dots carried a class no stylesheet
 * declared. Neither is visible in a diff; both are visible here.
 *
 * vitest's cwd is web/.
 */

const SRC = resolve(process.cwd(), 'src');

/**
 * The sheets that may define a token — §2 and §3.
 *
 * The palette moved out of `tokens.css` into vendored sheets, and it declares
 * on `body` rather than `:root` because the scrollbar sheet resolves against
 * the element the tokens sit on. So a definition is looked for across all of
 * these and under any selector, rather than in one file under one.
 */
const TOKEN_SHEETS = [
  'tokens.css',
  'design-platform.css',
  'effects-typography.css',
  'scrollbar.css',
  'shiki.css',
].map((name) => join(SRC, 'styles', name));
const COMPONENTS = join(SRC, 'styles', 'components.css');

function stylesheets(dir: string): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      found.push(...stylesheets(path));
    } else if (entry.name.endsWith('.css')) {
      found.push(path);
    }
  }
  return found.sort();
}

const ALL = stylesheets(SRC);
/*
 * Everything that is not a token sheet. The palette sheets are full of literal
 * colours by definition — that is what a palette is — so the no-literals rule
 * applies to the sheets that consume them, which is where a literal would
 * actually be unable to follow the theme.
 */
const COMPONENT_SHEETS = ALL.filter((path) => !TOKEN_SHEETS.includes(path));

/**
 * Custom properties a stylesheet may reference without defining.
 *
 * `--canvas-columns` is set inline from JavaScript in SessionCanvas, because
 * the column count is a layout fact the browser computes rather than a design
 * decision. Its CSS fallback is deliberate and must not be deleted.
 */
const SET_FROM_SCRIPT = new Set(['--canvas-columns']);

function captures(css: string, pattern: RegExp): Set<string> {
  const found = new Set<string>();
  for (const match of css.matchAll(pattern)) {
    const name = match[1];
    if (name !== undefined) {
      found.add(name);
    }
  }
  return found;
}

function declared(css: string): Set<string> {
  return captures(css, /(--[a-z0-9-]+)\s*:/g);
}

function referenced(css: string): Set<string> {
  return captures(css, /var\((--[a-z0-9-]+)/g);
}

describe('the token layer', () => {
  it('holds custom properties and nothing else', () => {
    // A class selector here would mean component styling had leaked back into
    // the sheets whose whole value is that checking §2 against them is a
    // single-place review. `@font-face` is allowed: the one face this product
    // downloads is declared beside the stack that names it.
    for (const sheet of TOKEN_SHEETS) {
      const withoutComments = readFileSync(sheet, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
      const selectors = [...withoutComments.matchAll(/([^{};]*)\{/g)]
        .map((match) => (match[1] ?? '').trim().split('\n').pop()?.trim() ?? '')
        .filter((selector) => selector.length > 0 && !selector.startsWith('@'));
      for (const selector of selectors) {
        expect(selector, `unexpected selector in ${relative(SRC, sheet)}: ${selector}`).toMatch(
          /^(:root|body|html|::?-?[a-z-]|\*)/,
        );
      }
    }
  });

  it('defines every themed colour in the light theme too', () => {
    // §3: a colour whose only definition lives in the dark block renders as
    // `unset` for every light reader — the same failure the old bare-`:root`
    // rule guarded, checked where the palette actually lives now.
    const { light, darkOverrides } = themes();

    expect(darkOverrides.size).toBeGreaterThan(0);
    for (const name of darkOverrides.keys()) {
      expect([...light.keys()], `--${name} is defined only in the dark theme`).toContain(name);
    }
  });

  it('resolves every token in both themes', () => {
    // A token whose chain ends on a name nothing defines renders as `unset`,
    // and does so only in the theme where the chain breaks — which is how one
    // survives review. Both themes are walked, every tier, to the literal.
    const palette = themes();
    for (const name of ['light', 'dark'] as const) {
      for (const token of palette[name].keys()) {
        expect(() => resolveToken(token, palette[name]), `--${token} in ${name}`).not.toThrow();
      }
    }
  });

  it('resolves every var() reference in every stylesheet', () => {
    const defined = new Set<string>();
    for (const sheet of TOKEN_SHEETS) {
      for (const name of declared(readFileSync(sheet, 'utf8'))) {
        defined.add(name);
      }
    }
    const missing: string[] = [];

    for (const path of ALL) {
      const css = readFileSync(path, 'utf8');
      const local = declared(css);
      for (const name of referenced(css)) {
        if (!defined.has(name) && !local.has(name) && !SET_FROM_SCRIPT.has(name)) {
          missing.push(`${relative(SRC, path)} references ${name}`);
        }
      }
    }

    expect(missing).toEqual([]);
  });
});

describe('the component layer', () => {
  it('declares no literal colour', () => {
    // §2: components reference tokens only. A literal here is a colour that
    // cannot follow the theme and will not appear in §2's contrast table.
    const offences: string[] = [];
    for (const path of COMPONENT_SHEETS) {
      for (const [index, line] of readFileSync(path, 'utf8').split('\n').entries()) {
        if (
          /#[0-9a-fA-F]{3,8}\b|\b(rgba?|hsla?)\(/.test(line) &&
          !line.trimStart().startsWith('*')
        ) {
          offences.push(`${relative(SRC, path)}:${index + 1} ${line.trim()}`);
        }
      }
    }
    expect(offences).toEqual([]);
  });

  it('declares no inline box-shadow', () => {
    // §3: the depth tokens are the only shadow values permitted. An inline
    // string is a value that cannot be checked against the letterpress system
    // and will not change with the theme.
    const offences: string[] = [];
    for (const path of COMPONENT_SHEETS) {
      for (const [index, line] of readFileSync(path, 'utf8').split('\n').entries()) {
        const match = /box-shadow:\s*(.+);/.exec(line);
        if (match && !(match[1] ?? '').startsWith('var(--')) {
          offences.push(`${relative(SRC, path)}:${index + 1} ${line.trim()}`);
        }
      }
    }
    expect(offences).toEqual([]);
  });

  it('gives every semantic tone a dot colour', () => {
    // SessionRail renders `tone-bg-${toneFor(lifecycle)}`, and until this
    // change no stylesheet declared those classes — every status dot in the
    // rail rendered with no colour at all. Recorded here rather than left to
    // be re-fixed, per issue #111.
    const css = readFileSync(COMPONENTS, 'utf8');
    for (const tone of ['live', 'wait', 'fail', 'ended']) {
      expect(css, `.tone-bg-${tone} is not declared`).toContain(`.tone-bg-${tone} {`);
    }
  });
});
