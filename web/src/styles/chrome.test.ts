import { readFileSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const SRC = resolve(process.cwd(), 'src');

function migrationShimNames(): Set<string> {
  const tokens = readFileSync(join(SRC, 'styles', 'tokens.css'), 'utf8');
  const migration = tokens.slice(tokens.indexOf('Migration shim'));
  const shimBody = /body\s*\{([\s\S]*?)\n\}/.exec(migration)?.[1] ?? '';
  const names = new Set(
    [...shimBody.matchAll(/^\s*(--[a-z0-9-]+):/gm)]
      .map((match) => match[1])
      .filter((name): name is string => name !== undefined),
  );
  names.delete('--focus-ring');
  return names;
}

describe('the migrated chrome sheets', () => {
  it('use no migration-shim property except the shared focus ring', () => {
    const shimNames = migrationShimNames();
    const sheets = [
      join(SRC, 'shell', 'CommandBar.css'),
      join(SRC, 'Console.css'),
      join(SRC, 'AppError.css'),
    ];
    const offences: string[] = [];

    for (const sheet of sheets) {
      const css = readFileSync(sheet, 'utf8');
      const references = [...css.matchAll(/var\((--[a-z0-9-]+)/g)]
        .map((match) => match[1])
        .filter((name): name is string => name !== undefined);
      for (const name of references.filter((candidate) => shimNames.has(candidate))) {
        offences.push(`${relative(SRC, sheet)} references ${name}`);
      }
    }

    expect(offences).toEqual([]);
  });

  it('keeps the console empty state quiet and its API code in the identifier face', () => {
    const css = readFileSync(join(SRC, 'Console.css'), 'utf8');
    expect(css).toMatch(
      /\.console__noticecode\s*\{[^}]*color: var\(--dsw-alias-label-tertiary\)[^}]*font-family: var\(--ds-font-family-code\)[^}]*font-size: 11px/,
    );
    expect(css).toMatch(
      /\.console__empty\s*\{[^}]*color: var\(--dsw-alias-label-tertiary\)[^}]*font-family: var\(--dsw-font-family\)[^}]*font-size: 12px/,
    );
  });

  it('keeps the unreachable screen composed and free of alarm colours', () => {
    const css = readFileSync(join(SRC, 'AppError.css'), 'utf8');
    expect(css).toMatch(/\.app-error\s*\{[^}]*background: var\(--dsw-alias-bg-base\)/);
    expect(css).toMatch(/\.app-error__what\s*\{[^}]*color: var\(--dsw-alias-label-primary\)/);
    expect(css).toMatch(/\.app-error__next\s*\{[^}]*color: var\(--dsw-alias-label-tertiary\)/);
    expect(css).not.toMatch(/var\(--dsw-alias-(?:state-error|interactive-bg-hover-danger)/);
  });
});
