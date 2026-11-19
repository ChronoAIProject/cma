import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { expect, it } from 'vitest';
import { signInPath, silentReturnTarget } from './nyxid';

const cases = JSON.parse(
  readFileSync(
    resolve(
      dirname(fileURLToPath(import.meta.url)),
      '../../../api/crates/layer1-public/tests/fixtures/browser-auth-return-targets.json',
    ),
    'utf8',
  ),
) as { target: string; explicit: string | null; backend: boolean; silent: string | null }[];

it.each(cases)('emits a server-supported explicit destination for $target', (item) => {
  for (const options of [undefined, { screen: 'register' as const }]) {
    if (item.explicit === null) {
      expect(() => signInPath(item.target, options)).toThrow();
      continue;
    }
    const login = new URL(signInPath(item.target, options), window.location.origin);
    const emitted = login.searchParams.get('return_to');
    expect(emitted).toBe(item.explicit);
    expect(cases.find((candidate) => candidate.target === emitted)?.backend).toBe(true);
    expect(login.searchParams.get('screen')).toBe(options?.screen ?? null);
    expect(login.searchParams.has('prompt')).toBe(false);
  }
});

it.each(cases)('keeps silent eligibility separate for $target', (item) => {
  const index = item.target.indexOf('?');
  const pathname = index < 0 ? item.target : item.target.slice(0, index);
  const search = index < 0 ? '' : item.target.slice(index);
  expect(silentReturnTarget({ pathname, search, hash: '' }) ?? null).toBe(item.silent);
});
