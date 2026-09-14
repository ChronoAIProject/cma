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
const TOKENS = readFileSync(join(STYLES, 'tokens.css'), 'utf8');

/** Every stylesheet, so a face moving between sheets cannot escape these. */
function styleSheets(): string[] {
  return readdirSync(STYLES, { recursive: true, encoding: 'utf8' })
    .filter((entry) => entry.endsWith('.css'))
    .map((entry) => readFileSync(join(STYLES, entry), 'utf8'));
}

/** Every declaration block for one selector, in source order. */
function blocks(css: string, selector: string): string[] {
  const escaped = selector.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  return [...css.matchAll(new RegExp(`${escaped}\\s*\\{([^}]*)\\}`, 'g'))].map(
    (match) => match[1] ?? '',
  );
}

/** A custom property's whitespace-normalised value from the first block that declares it. */
function property(source: readonly string[], name: string): string {
  for (const block of source) {
    const value = new RegExp(`${name}:\\s*([^;]+);`).exec(block)?.[1];
    if (value !== undefined) {
      return value.replace(/\s+/g, ' ').trim();
    }
  }
  return '';
}

function stack(value: string): string[] {
  return value.split(',').map((family) => family.trim());
}

describe('the platform faces', () => {
  it('vendors the five selected families, their seven faces, and nothing else', () => {
    // Exact equality: a retired face lingering here would still ship dead bytes.
    expect(readdirSync(FONTS).sort()).toEqual([
      'Archivo-Variable.woff2',
      'FamiljenGrotesk-Variable.woff2',
      'IBMPlexMono-Medium.woff2',
      'IBMPlexMono-Regular.woff2',
      'IBMPlexMono-SemiBold.woff2',
      'JetBrainsMono-Variable.woff2',
      'OFL-Archivo.txt',
      'OFL-FamiljenGrotesk.txt',
      'OFL-IBMPlexMono.txt',
      'OFL-JetBrainsMono.txt',
      'OFL-Shrikhand.txt',
      'Shrikhand-Regular.woff2',
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

  it('declares every face with its exact file, weight, format, and swap behaviour', () => {
    const faces = styleSheets().flatMap((css) =>
      [...css.matchAll(/@font-face\s*\{([^}]*)\}/g)].map((match) => match[1] ?? ''),
    );
    expect(faces).toHaveLength(7);

    const declarations = faces.map((face) => ({
      family: /font-family:\s*'([^']+)'/.exec(face)?.[1] ?? '',
      file: /url\('\.\.\/assets\/fonts\/([^']+)'\)/.exec(face)?.[1] ?? '',
      format: /format\('([^']+)'\)/.exec(face)?.[1] ?? '',
      weight: /font-weight:\s*([^;]+)/.exec(face)?.[1]?.trim() ?? '',
      display: /font-display:\s*([^;]+)/.exec(face)?.[1]?.trim() ?? '',
    }));
    expect(declarations).toEqual([
      {
        family: 'Shrikhand',
        file: 'Shrikhand-Regular.woff2',
        format: 'woff2',
        weight: '400',
        display: 'swap',
      },
      {
        family: 'Archivo',
        file: 'Archivo-Variable.woff2',
        format: 'woff2-variations',
        weight: '100 900',
        display: 'swap',
      },
      {
        family: 'Familjen Grotesk',
        file: 'FamiljenGrotesk-Variable.woff2',
        format: 'woff2-variations',
        weight: '400 700',
        display: 'swap',
      },
      {
        family: 'IBM Plex Mono',
        file: 'IBMPlexMono-Regular.woff2',
        format: 'woff2',
        weight: '400',
        display: 'swap',
      },
      {
        family: 'IBM Plex Mono',
        file: 'IBMPlexMono-Medium.woff2',
        format: 'woff2',
        weight: '500',
        display: 'swap',
      },
      {
        family: 'IBM Plex Mono',
        file: 'IBMPlexMono-SemiBold.woff2',
        format: 'woff2',
        weight: '600',
        display: 'swap',
      },
      {
        family: 'JetBrains Mono',
        file: 'JetBrainsMono-Variable.woff2',
        format: 'woff2-variations',
        weight: '100 800',
        display: 'swap',
      },
    ]);

    const files = readdirSync(FONTS);
    for (const declaration of declarations) {
      expect(files).toContain(declaration.file);
    }
  });

  it('switches both branded stacks on body while retaining deliberate fallbacks', () => {
    const light = blocks(TOKENS, 'body');
    const dark = blocks(TOKENS, 'body[data-ds-dark-theme]');
    const root = blocks(TOKENS, ':root').join('\n');
    expect(root).not.toContain('--dsw-font-family');
    expect(root).not.toContain('--ds-font-family-code');

    const lightUi = stack(property(light, '--dsw-font-family'));
    const darkUi = stack(property(dark, '--dsw-font-family'));
    const lightCode = stack(property(light, '--ds-font-family-code'));
    const darkCode = stack(property(dark, '--ds-font-family-code'));

    expect(lightUi).toEqual([
      "'Familjen Grotesk'",
      "'Noto Sans SC'",
      '-apple-system',
      'BlinkMacSystemFont',
      "'Segoe UI'",
      "'PingFang SC'",
      "'Hiragino Sans GB'",
      "'Microsoft YaHei'",
      'system-ui',
      "'Helvetica Neue'",
      'Helvetica',
      'Arial',
      'sans-serif',
    ]);
    expect(darkUi).toEqual(["'Archivo'", ...lightUi.slice(1)]);
    expect(lightCode).toEqual([
      "'IBM Plex Mono'",
      "'SF Mono'",
      "'JetBrains Mono'",
      "'Fira Code'",
      'Consolas',
      "'Liberation Mono'",
      'Menlo',
      'Courier',
      "'PingFang SC'",
      "'Microsoft YaHei'",
    ]);
    expect(darkCode).toEqual(["'JetBrains Mono'", "'SF Mono'", ...lightCode.slice(3)]);

    const sans = ["'Archivo'", "'Familjen Grotesk'"];
    const mono = ["'IBM Plex Mono'", "'JetBrains Mono'"];
    for (const ui of [lightUi, darkUi]) {
      expect(ui.filter((family) => sans.includes(family))).toHaveLength(1);
      expect(ui.some((family) => mono.includes(family))).toBe(false);
      expect(ui).toContain('-apple-system');
    }
    for (const code of [lightCode, darkCode]) {
      expect(code).not.toContain('monospace');
    }

    const everyStack = [...lightUi, ...darkUi, ...lightCode, ...darkCode];
    for (const family of [...sans, ...mono]) {
      expect(everyStack).toContain(family);
    }
  });

  it('keeps the display width conventional in light and widens Archivo in dark', () => {
    const effects = readFileSync(join(STYLES, 'effects-typography.css'), 'utf8');
    expect(property(blocks(effects, 'body'), '--ds-font-variation-display')).toBe('normal');
    expect(
      property(blocks(effects, 'body[data-ds-dark-theme]'), '--ds-font-variation-display'),
    ).toBe("'wdth' 118");
  });

  it('licenses every family under OFL 1.1 with its own copyright line', () => {
    const licenses = [
      ['OFL-Archivo.txt', 'The Archivo Project Authors'],
      ['OFL-FamiljenGrotesk.txt', 'The Familjen Grotesk Project Authors'],
      ['OFL-IBMPlexMono.txt', 'Copyright \u00a9 2017 IBM Corp.'],
      ['OFL-JetBrainsMono.txt', 'JetBrains Mono Project Authors'],
      ['OFL-Shrikhand.txt', 'Jonny Pinhorn'],
    ] as const;
    expect(
      readdirSync(FONTS)
        .filter((file) => file.startsWith('OFL-'))
        .sort(),
    ).toEqual(licenses.map(([file]) => file).sort());
    for (const [file, copyright] of licenses) {
      const text = readFileSync(join(FONTS, file), 'utf8');
      expect(text).toContain('SIL OPEN FONT LICENSE Version 1.1');
      expect(text).toContain(copyright);
    }
  });
});
