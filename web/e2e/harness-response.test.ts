// @vitest-environment node
import { EventEmitter } from 'node:events';
import { mkdirSync, rmSync } from 'node:fs';
import type { Browser } from '@playwright/test';
import { afterEach, expect, it, vi } from 'vitest';
import { Harness } from './harness';
import type { Hooks } from './environment';
import { fixtureDocument } from './environment.fixture';

const directories: string[] = [];
afterEach(() => {
  vi.unstubAllEnvs();
  directories.splice(0).forEach((directory) => rmSync(directory, { recursive: true }));
});

async function observe(
  method: string,
  body: unknown,
  failure?: string,
  unreadable = false,
  version = 'v1',
) {
  const fixture = fixtureDocument();
  directories.push(fixture.directory);
  for (const [key, value] of Object.entries(fixture.env)) vi.stubEnv(key, value);
  mkdirSync(fixture.env.CMA_E2E_OUTPUT_DIR);
  const context = Object.assign(new EventEmitter(), {
    request: {
      get: async () => ({
        ok: () => true,
        json: async () => ({
          authenticated: true,
          subject: fixture.document.visitors['J4-oauth']!.subject,
        }),
      }),
    },
    route: async () => {},
    newPage: async () => ({}),
  });
  const h = new Harness(
    { newContext: async () => context } as unknown as Browser,
    {} as Hooks,
    'J4-oauth',
  );
  await h.open('visitor');
  context.emit('response', {
    url: () =>
      fixture.document.origin +
      (method === 'GET'
        ? '/api/v1/profile-authoring/services'
        : `/api/${version}/triggers/public/fixture/launches`),
    request: () => ({
      method: () => method,
      failure: () => (failure === undefined ? null : { errorText: failure }),
    }),
    status: () => (method === 'GET' ? 200 : 201),
    headers: () => ({ 'content-type': 'application/json' }),
    text: async () => {
      if (unreadable) throw Error('Response body unavailable');
      return JSON.stringify(body);
    },
  });
  await h.flush();
  return h;
}

it('inspects list responses without treating catalog rows as owned resources', async () => {
  const h = await observe('GET', [{ id: 'existing-service' }]);
  expect(h.violations).toEqual([]);
  expect(h.journal.resources).toEqual([]);
});
it.each(['v1', 'v2'])(
  'does not accept array responses as %s command admission receipts',
  async (version) => {
    const h = await observe('POST', [{ id: 'claimed-launch' }], undefined, false, version);
    expect(h.violations).toEqual(['response_inspection_failed']);
    expect(h.journal.resources).toEqual([]);
  },
);
it.each(['v1', 'v2'])('journals %s Launch responses from the browser observer', async (version) => {
  const h = await observe('POST', { id: 'created-launch' }, undefined, false, version);
  expect(h.violations).toEqual([]);
  expect(h.journal.resources).toEqual([
    { kind: 'launches', id: 'created-launch', subject: h.fixture.accounts.visitor.subject },
  ]);
});
it('keeps credential checks on catalog responses', async () => {
  expect((await observe('GET', ['redacted-probe'])).violations).toEqual([
    'response_inspection_failed',
  ]);
});
it('does not treat a browser-cancelled GET as a completed response', async () => {
  const h = await observe('GET', {}, 'net::ERR_ABORTED', true);
  expect(h.violations).toEqual([]);
  expect(h.journal.resources).toEqual([]);
});
for (const [method, failure] of [
  ['POST', 'net::ERR_ABORTED'],
  ['GET', 'net::ERR_FAILED'],
  ['GET', undefined],
] as const)
  it(`refuses an unreadable ${method} response with ${failure ?? 'no browser failure'}`, async () => {
    expect((await observe(method, {}, failure, true)).violations).toEqual([
      'response_inspection_failed',
    ]);
  });

it('still rejects credentials when a cancelled read has a body', async () => {
  expect((await observe('GET', ['redacted-probe'], 'net::ERR_ABORTED')).violations).toEqual([
    'response_inspection_failed',
  ]);
});
