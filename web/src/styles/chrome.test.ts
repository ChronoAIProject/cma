import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { migrationShimOffences } from '../test/migrationShim';

const SRC = resolve(process.cwd(), 'src');

describe('the migrated chrome sheets', () => {
  it('use no migration-shim property except the shared focus ring', () => {
    const sheets = [join(SRC, 'Console.css'), join(SRC, 'AppError.css')];
    expect(migrationShimOffences(sheets)).toEqual([]);
  });

  it('keeps the console empty state quiet and its eyebrow in the shared label voice', () => {
    const css = readFileSync(join(SRC, 'Console.css'), 'utf8');
    expect(css).toMatch(
      /\.console__eyebrow\s*\{[^}]*color: var\(--dsw-alias-label-tertiary\)[^}]*font: var\(--dsw-font-xxs-strong-12\)/,
    );
    expect(css).not.toMatch(/\.console__eyebrow\s*\{[^}]*text-transform: uppercase/);
    expect(css).toMatch(/\.console__empty\s*\{[^}]*max-width: 32rem[^}]*text-align: center/);
  });

  it('keeps the unreachable screen composed and free of alarm colours', () => {
    const css = readFileSync(join(SRC, 'AppError.css'), 'utf8');
    expect(css).toMatch(/\.app-error\s*\{[^}]*background: var\(--dsw-alias-bg-base\)/);
    expect(css).toMatch(/\.app-error__title\s*\{[^}]*color: var\(--dsw-alias-label-primary\)/);
    expect(css).toMatch(/\.app-error__next\s*\{[^}]*color: var\(--dsw-alias-label-secondary\)/);
    expect(css).toMatch(/\.app-error__what\s*\{[^}]*color: var\(--dsw-alias-label-tertiary\)/);
    expect(css).not.toMatch(/var\(--dsw-alias-(?:state-error|interactive-bg-hover-danger)/);
  });
});
