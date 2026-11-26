// @vitest-environment node
import { rmSync, writeFileSync } from 'node:fs';
import { afterEach, expect, it, vi } from 'vitest';
import { fixtures, hooks } from './environment';
import { fixtureDocument } from './environment.fixture';

const directories: string[] = [];
afterEach(() => {
  vi.restoreAllMocks();
  directories.splice(0).forEach((directory) => rmSync(directory, { recursive: true }));
});
function setup() {
  const fixture = fixtureDocument();
  directories.push(fixture.directory);
  return fixture;
}

it('binds all connection mode fixtures to the pinned identity provider before running', () => {
  const fixture = setup();
  expect(fixtures(fixture.env).connectionJourneys).toEqual(fixture.document.connectionJourneys);
  for (const mode of ['oauth', 'api_key', 'device_code']) {
    for (const change of [
      { providerRevision: 'a'.repeat(40) },
      { serviceId: '' },
      { fixtureDigest: 'unreviewed' },
    ]) {
      const document = structuredClone(fixture.document);
      Object.assign(document.connectionJourneys.services[mode]!, change);
      writeFileSync(fixture.manifest, JSON.stringify(document));
      expect(() => fixtures(fixture.env)).toThrow();
    }
  }
  const { connectionJourneys: _removed, ...missing } = fixture.document;
  expect(_removed).toBeDefined();
  writeFileSync(fixture.manifest, JSON.stringify(missing));
  expect(() => fixtures(fixture.env)).toThrow();
});

it('refuses missing observation capability without executing any operator hook', async () => {
  const fixture = setup();
  writeFileSync(
    fixture.document.hooksFile,
    ['completeSignup', 'completeConnection', 'publishEmbed', 'controlFault', 'verifyRuntime']
      .map((name) => `export async function ${name}() { throw new Error('HOOK_EXECUTED'); }`)
      .join('\n'),
  );
  await expect(hooks(fixtures(fixture.env))).rejects.toThrow('observeConnection');
});

it('loads observation capability without invoking it during preflight', async () => {
  const fixture = setup();
  writeFileSync(
    fixture.document.hooksFile,
    [
      'completeSignup',
      'completeConnection',
      'publishEmbed',
      'controlFault',
      'verifyRuntime',
      'observeConnection',
    ]
      .map((name) => `export async function ${name}() { throw new Error('HOOK_EXECUTED'); }`)
      .join('\n'),
  );
  expect(typeof (await hooks(fixtures(fixture.env))).observeConnection).toBe('function');
});
