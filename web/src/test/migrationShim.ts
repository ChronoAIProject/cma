import { readFileSync } from 'node:fs';
import { relative, resolve } from 'node:path';

const SRC = resolve(process.cwd(), 'src');

export function migrationShimOffences(
  sheets: readonly string[],
  exemptions: readonly string[] = ['--focus-ring'],
): string[] {
  const tokens = readFileSync(resolve(SRC, 'styles', 'tokens.css'), 'utf8');
  const migration = tokens.slice(tokens.indexOf('Migration shim'));
  const shimBody = /body\s*\{([\s\S]*?)\n\}/.exec(migration)?.[1] ?? '';
  const shimNames = new Set(
    [...shimBody.matchAll(/^\s*(--[a-z0-9-]+):/gm)]
      .map((match) => match[1])
      .filter((name): name is string => name !== undefined),
  );
  for (const exemption of exemptions) {
    shimNames.delete(exemption);
  }

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
  return offences;
}
