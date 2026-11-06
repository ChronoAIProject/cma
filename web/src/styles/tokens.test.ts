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
 * `--canvas-columns` is reserved for a future resource canvas, because
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
  it('draws selected text with the appearance accent and paired foreground', () => {
    const base = readFileSync(join(SRC, 'styles', 'base.css'), 'utf8');
    const selection = /::selection\s*\{([^}]*)\}/.exec(base)?.[1] ?? '';
    expect(selection).toMatch(/color:\s*var\(--dsw-alias-label-primary-foreground\)/);
    expect(selection).toMatch(/background:\s*var\(--dsw-alias-state-business-primary\)/);
  });

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

  it('declares no hand-rolled depth', () => {
    /*
     * §3: depth comes from the shadow tokens. A hand-written offset and blur
     * is a value that cannot be checked against the elevation system and will
     * not change with the theme.
     *
     * An `inset` ring is exempt and is not an exception to the rule — it is a
     * different thing wearing the same property. It draws a hairline without
     * changing the element's box, which is why the selected pill uses one
     * instead of a border that would shift its neighbours by a pixel as the
     * selection moves. Its colour must still be a token.
     */
    const offences: string[] = [];
    for (const path of COMPONENT_SHEETS) {
      for (const [index, line] of readFileSync(path, 'utf8').split('\n').entries()) {
        const match = /box-shadow:\s*(.+);/.exec(line);
        const value = match?.[1];
        /*
         * `none` is an opt-out, not a hand-rolled depth: removing a shadow
         * cannot introduce a value the elevation system has not seen.
         */
        if (value === undefined || value.startsWith('var(--') || value === 'none') {
          continue;
        }
        const ring = /^inset\s+[\d\s.px-]+\s+var\(--[a-z0-9-]+\)$/.test(value);
        if (!ring) {
          offences.push(`${relative(SRC, path)}:${index + 1} ${line.trim()}`);
        }
      }
    }
    expect(offences).toEqual([]);
  });

  it('gives every state the dot can take a colour', () => {
    /*
     * The rail once rendered `tone-bg-${lifecycle}` classes that no stylesheet
     * declared, so every status dot in it drew with no colour at all — issue
     * #111. The classes are gone and StateDot owns the colours now, so the
     * guard moves with them rather than retiring.
     *
     * `ongoing` has the same state attribute as the static dots. Its core uses
     * that colour while the one child ring supplies the liveness motion.
     */
    const css = readFileSync(join(SRC, 'ui', 'StateDot.css'), 'utf8');
    for (const state of ['done', 'warning', 'error']) {
      expect(css, `no colour for the ${state} state`).toMatch(
        new RegExp(`\\[data-state='${state}'\\]\\s*\\{[^}]*color: var\\(--`),
      );
    }
    expect(css, 'the running state has no colour').toMatch(
      /\.statedot\[data-state='ongoing'\]\s*\{[^}]*color: var\(--/,
    );
  });
});

describe('motion', () => {
  it('removes motion on request rather than adding it on permission', () => {
    /*
     * `prefers-reduced-motion: no-preference` matches only where somebody has
     * explicitly expressed none. A platform that does not report the feature
     * at all matches *neither* value — so gating motion on `no-preference`
     * silently took every hover fill and every caret away from those people,
     * which is the opposite of what the gate was for.
     *
     * The `reduce` override is the idiom that gets this right: motion is the
     * default, and asking for less removes it.
     */
    const offences: string[] = [];
    for (const path of ALL) {
      const css = readFileSync(path, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
      if (css.includes('prefers-reduced-motion: no-preference')) {
        offences.push(relative(SRC, path));
      }
    }
    expect(offences).toEqual([]);
  });

  it('gives every looping animation a way to stop', () => {
    /*
     * A transition happens once and is over; an `infinite` animation does
     * not, and is exactly what a reduced-motion request is about. Every sheet
     * that starts one has to say how it stops.
     */
    const offences: string[] = [];
    for (const path of ALL) {
      const css = readFileSync(path, 'utf8');
      if (/animation:[^;]*infinite/.test(css) && !css.includes('prefers-reduced-motion: reduce')) {
        offences.push(relative(SRC, path));
      }
    }
    expect(offences).toEqual([]);
  });

  it('eases on one curve, from the tokens', () => {
    // §4: one curve, three durations. A hand-written cubic-bezier is a fourth
    // curve nobody chose.
    const offences: string[] = [];
    for (const path of COMPONENT_SHEETS) {
      const css = readFileSync(path, 'utf8');
      if (/cubic-bezier\(/.test(css)) {
        offences.push(relative(SRC, path));
      }
    }
    expect(offences).toEqual([]);
  });
});

describe('the scope a token is declared in', () => {
  /**
   * Which selector each property is declared under, across the token sheets.
   *
   * Blocks are matched on their own line-start selector, which is enough here
   * because these sheets are flat: every declaration sits directly under
   * `:root`, `body`, or `body[data-ds-dark-theme]`.
   */
  function declarationsBySelector(): Map<string, Set<string>> {
    const found = new Map<string, Set<string>>();
    for (const sheet of TOKEN_SHEETS) {
      const css = readFileSync(sheet, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
      for (const block of css.matchAll(/(?:^|\n)(\s*)([^{}\n]+?)\s*\{([\s\S]*?)\n\1\}/g)) {
        const selector = (block[2] ?? '').trim();
        for (const declaration of (block[3] ?? '').matchAll(/(--[\w-]+):\s*([^;]+);/g)) {
          const under = found.get(selector) ?? new Set<string>();
          under.add(declaration[1] ?? '');
          found.set(selector, under);
        }
      }
    }
    return found;
  }

  it('never references a property its own scope cannot see', () => {
    /*
     * The bug this exists for, found in a real browser and invisible to every
     * other test here: a custom property's value is substituted in the scope
     * it is **declared** in, not the scope it is used from.
     *
     * So `:root { --ground: var(--dsw-alias-bg-base) }` resolves against
     * `:root` — where the palette, declared on `body`, does not exist — and
     * computes to nothing. Every old name went empty, and the whole page
     * rendered on browser defaults: no ground, no ink, no rules, no focus
     * ring. jsdom resolves no custom properties, so 1083 tests passed.
     *
     * `:root` is an ancestor of `body`, so `body` may reference what `:root`
     * declares. The reverse is what breaks.
     */
    const bySelector = declarationsBySelector();
    const onRoot = bySelector.get(':root') ?? new Set<string>();
    const onBody = new Set([
      ...(bySelector.get('body') ?? []),
      ...(bySelector.get('body[data-ds-dark-theme]') ?? []),
    ]);

    const offences: string[] = [];
    for (const sheet of TOKEN_SHEETS) {
      const css = readFileSync(sheet, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
      for (const block of css.matchAll(/(?:^|\n)(\s*):root\s*\{([\s\S]*?)\n\1\}/g)) {
        for (const declaration of (block[2] ?? '').matchAll(/(--[\w-]+):\s*([^;]+);/g)) {
          for (const reference of (declaration[2] ?? '').matchAll(/var\(\s*(--[\w-]+)/g)) {
            const name = reference[1] ?? '';
            const property = declaration[1] ?? '';
            /*
             * A `:root` declaration that cannot resolve is harmless once the
             * same property is declared on `body` as well — everything under
             * the body then reads the one that works. What is being caught is
             * a property with no resolvable declaration at all.
             */
            if (!onRoot.has(name) && onBody.has(name) && !onBody.has(property)) {
              offences.push(
                `${relative(SRC, sheet)}: ${property} on :root reads ${name}, which only body declares`,
              );
            }
          }
        }
      }
    }
    expect(offences).toEqual([]);
  });
});

describe('New agent landing palette', () => {
  it.each([
    ['page', '#ffffff', '#0b0b0c'],
    ['shell', '#fafafa', '#131315'],
    ['panel', '#ffffff', '#19191c'],
    ['accent', '#4d7c0f', '#c8f542'],
    ['accent-hover', '#d7ff5a', '#d7ff5a'],
    ['on-accent', '#0b0b0c', '#0b0b0c'],
    ['muted-icon', '#71717a', '#71717a'],
    ['primary', '#111113', '#f4f4f5'],
    ['placeholder', '#52525b', '#85858e'],
    ['border', '#e4e4e7', '#26262a'],
    ['panel-border', '#e4e4e7', '#2a2a2e'],
    ['focus-border', '#a1a1aa', '#3a3a40'],
    ['disabled', '#e4e4e7', '#26262a'],
    ['menu', '#ffffff', '#18181b'],
    ['hover', '#f4f4f5', '#1c1c1f'],
    ['button-fill', '#c8f542', '#c8f542'],
  ])('resolves task %s in light and dark', (suffix, light, dark) => {
    const palette = themes();
    expect(resolveToken(`dsw-specific-task-${suffix}`, palette.light)).toBe(light);
    expect(resolveToken(`dsw-specific-task-${suffix}`, palette.dark)).toBe(dark);
  });
});

it('shares app roles only for task page, text, accent and primary button paints', () => {
  const palette = themes();
  for (const theme of ['light', 'dark'] as const)
    for (const suffix of [
      'page',
      'primary',
      'weak',
      'accent',
      'accent-hover',
      'on-accent',
      'button-fill',
    ])
      expect(palette[theme].get(`dsw-specific-task-${suffix}`)).toMatch(/^var\(--dsw-alias-/);
});

it('retains subtle global borders and non-primary button fill families', () => {
  const palette = themes();
  for (const [suffix, light, dark] of [
    ['border-l3', '#d4d4d8', '#3f3f46'],
    ['border-l4', '#a1a1aa', '#52525b'],
    ['button-info-fill', '#18181b', '#c3fd43'],
    ['button-info-hover', '#27272a', '#aee734'],
  ]) {
    expect(resolveToken(`dsw-alias-${suffix}`, palette.light)).toBe(light);
    expect(resolveToken(`dsw-alias-${suffix}`, palette.dark)).toBe(dark);
  }
});
