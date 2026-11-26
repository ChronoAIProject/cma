// @vitest-environment node
import { mkdtempSync, mkdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { createServer, request } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import type { Browser } from '@playwright/test';
import { expect, it } from 'vitest';
import { connectionWizardFixture } from './connection-journey-ui.fixture';

function read(origin: string, target: string) {
  return new Promise<{ status: number; body: string }>((resolve, reject) => {
    const operation = request(origin, { path: target }, (response) => {
      let body = '';
      response.setEncoding('utf8');
      response.on('data', (chunk) => {
        body += chunk;
      });
      response.on('end', () => resolve({ status: response.statusCode!, body }));
    });
    operation.on('error', reject);
    operation.end();
  });
}

it('serves only preloaded build assets and known app routes without forwarding request targets', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'cma-wizard-assets-'));
  const build = join(directory, 'build');
  mkdirSync(join(build, 'assets'), { recursive: true });
  writeFileSync(join(build, 'index.html'), '<title>Compiled fixture</title>');
  writeFileSync(join(build, 'assets', 'app.js'), 'export const compiled = true;');
  writeFileSync(join(directory, 'private.txt'), 'OUTSIDE_BUILD_SENTINEL');
  symlinkSync(join(directory, 'private.txt'), join(build, 'linked.txt'));
  let forwarded = 0;
  const upstream = createServer((_request, response) => {
    forwarded += 1;
    response.end('FORWARDED_SENTINEL');
  });
  await new Promise<void>((resolve) => upstream.listen(0, '127.0.0.1', resolve));
  const address = upstream.address();
  if (!address || typeof address === 'string') throw Error('Missing loopback fixture port');
  const preview = `http://127.0.0.1:${address.port}`;
  const fixture = await connectionWizardFixture({} as Browser, preview, 'J4-oauth', build);
  try {
    const origin = fixture.h.config.baseURL;
    for (const target of [preview + '/private', '//127.0.0.1/private']) {
      expect(await read(origin, target)).toEqual({ status: 400, body: '' });
      expect(forwarded).toBe(0);
    }
    for (const path of [
      '/index.html',
      '/launches/launch-J4-oauth',
      '/connection-return?status=completed',
    ])
      expect(await read(origin, path)).toEqual({
        status: 200,
        body: '<title>Compiled fixture</title>',
      });
    expect(await read(origin, '/assets/app.js')).toEqual({
      status: 200,
      body: 'export const compiled = true;',
    });
    for (const path of ['/missing.js', '/linked.txt', '/../private.txt', '/%2e%2e/private.txt'])
      expect(await read(origin, path)).toEqual({ status: 404, body: '' });
    expect(forwarded).toBe(0);
  } finally {
    await fixture.close();
    await new Promise<void>((resolve, reject) => {
      upstream.close((error) => (error ? reject(error) : resolve()));
      upstream.closeAllConnections();
    });
    rmSync(directory, { recursive: true, force: true });
  }
});
