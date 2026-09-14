import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

/**
 * Reading the design tokens the way the browser does — for tests only.
 *
 * Token tests need to answer "what colour is this token, in this theme" without
 * a browser. This reader keeps that source parsing in one place.
 */

/** Read from disk: vitest does not run Vite's CSS pipeline, so `?raw` is empty. */
export function sheet(name: string): string {
  return readFileSync(resolve(process.cwd(), 'src/styles', name), 'utf8');
}

/**
 * Every declaration under a selector, later blocks winning over earlier.
 *
 * **All** matching blocks, deliberately. The palette declares its three tiers
 * in three separate `body` blocks, and a reader that stopped at the first one
 * would check the static ramp and silently skip every alias above it — which
 * is precisely the bug this replaced.
 */
export function declarationsUnder(css: string, selector: string): Map<string, string> {
  const found = new Map<string, string>();
  const blocks = new RegExp(`(?:^|\\n)${selector}\\s*\\{([\\s\\S]*?)\\n\\}`, 'g');
  for (const block of css.matchAll(blocks)) {
    for (const declaration of (block[1] ?? '').matchAll(/--([\w-]+):\s*([^;]+);/g)) {
      found.set(declaration[1] ?? '', (declaration[2] ?? '').trim());
    }
  }
  return found;
}

export interface Themes {
  readonly light: Map<string, string>;
  readonly dark: Map<string, string>;
  /** What the dark theme restates for itself, rather than what it ends up with. */
  readonly darkOverrides: Map<string, string>;
}

/**
 * The palette as the two themes actually resolve it.
 *
 * The dark theme redefines only what changes, so it inherits the light block —
 * exactly as the cascade does at runtime, and the reason a token absent from
 * the dark block is not a token absent from the dark theme.
 */
export function themes(name = 'design-platform.css'): Themes {
  const css = sheet(name);
  const light = declarationsUnder(css, 'body');
  const darkOverrides = declarationsUnder(css, 'body\\[data-ds-dark-theme\\]');
  return { light, dark: new Map([...light, ...darkOverrides]), darkOverrides };
}

/**
 * Resolve a token through to the literal it names.
 *
 * The palette has three tiers — a static ramp, an alias over it, a specific
 * over that — so reading one level would yield `var(--dsw-static-…)` and
 * compare a name against a colour, which passes only by never matching.
 *
 * Throws rather than returning the name: a token that does not resolve is a
 * token that renders as `unset`, and a test that quietly compared two unset
 * values would report the drift it exists to catch as agreement.
 */
export function resolveToken(name: string, source: Map<string, string>): string {
  let current = name;
  for (let hop = 0; hop < 8; hop += 1) {
    const value = source.get(current);
    if (value === undefined) {
      throw new Error(`--${current} is not defined`);
    }
    const indirect = /^var\(\s*--([\w-]+)\s*\)$/.exec(value);
    if (indirect?.[1] === undefined) {
      return value;
    }
    current = indirect[1];
  }
  throw new Error(`--${name} does not resolve within eight hops — a token cycle`);
}

/** The sheet writes `rgb(r, g, b)`; xterm takes hex. Compare in one form. */
export function asHex(value: string): string {
  const channels = /^rgba?\(([^)]+)\)$/.exec(value);
  if (channels?.[1] === undefined) {
    return value.trim().toUpperCase();
  }
  return `#${channels[1]
    .split(/[,/\s]+/)
    .filter((part) => part.length > 0)
    .slice(0, 3)
    .map((part) => Number(part).toString(16).padStart(2, '0'))
    .join('')}`.toUpperCase();
}
