import { readdirSync, readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

/** @vitest-environment node */

const LOCAL = new URL('fixtures/', import.meta.url);
const SERVER = new URL(
  '../../../api/crates/layer1-public/tests/fixtures/rest-sdk/',
  import.meta.url,
);

describe('shared REST fixtures', () => {
  it('is byte-identical to the authoritative server corpus', () => {
    const localNames = readdirSync(LOCAL).sort();
    const serverNames = readdirSync(SERVER).sort();
    expect(localNames).toEqual(serverNames);

    for (const name of serverNames) {
      expect(readFileSync(new URL(name, LOCAL))).toEqual(readFileSync(new URL(name, SERVER)));
    }
  });
});
