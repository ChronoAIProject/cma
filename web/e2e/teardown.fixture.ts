import { createServer, type ServerResponse } from 'node:http';
import { mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import type { Browser, Page } from '@playwright/test';
import { Harness } from './harness';
import { fixtureDocument } from './environment.fixture';
import type { Hooks } from './environment';

export async function teardownFixture() {
  const fixture = fixtureDocument();
  mkdirSync(fixture.env.CMA_E2E_OUTPUT_DIR);
  writeFileSync(
    fixture.document.hooksFile,
    [
      'completeSignup',
      'completeConnection',
      'observeConnection',
      'publishEmbed',
      'controlFault',
      'verifyRuntime',
    ]
      .map((name) => `export function ${name}() { throw Error('Unused synthetic hook'); }`)
      .join('\n'),
  );
  const subject = fixture.document.visitors['J4-oauth']!.subject;
  const requests: string[] = [];
  let pending: ServerResponse | undefined;
  let removed = false;
  let anonymous = false;
  let signalRequest!: () => void;
  const received = new Promise<void>((resolve) => {
    signalRequest = resolve;
  });
  const server = createServer((request, response) => {
    const path = request.url ?? '';
    if (path.startsWith('/api/v1/held')) {
      pending = response;
      if (path !== '/api/v1/held-no-headers') {
        response.writeHead(200, { 'Content-Type': 'application/json' });
        response.write('[');
      }
      signalRequest();
      return;
    }
    if (path === '/api/v1/sandboxes') {
      pending = response;
      response.writeHead(201, { 'Content-Type': 'application/json' });
      response.write('{');
      signalRequest();
      return;
    }
    if (path === '/api/v1/triggers/owned-trigger') {
      requests.push(request.method!);
      if (request.method === 'DELETE') removed = true;
      response.writeHead(removed && request.method === 'GET' ? 404 : 200, {
        'Content-Type': 'application/json',
      });
      response.end(JSON.stringify({ id: 'owned-trigger', owner: subject, availability: 'draft' }));
      return;
    }
    const body =
      path === '/auth/session'
        ? anonymous
          ? { authenticated: false }
          : { authenticated: true, subject }
        : path === '/api/v1/triggers'
          ? { id: 'owned-trigger', owner: subject, availability: 'draft' }
          : path === '/api/v1/catalog'
            ? []
            : undefined;
    response.writeHead(path === '/api/v1/triggers' ? 201 : 200, {
      'Content-Type': body ? 'application/json' : 'text/html',
    });
    response.end(body ? JSON.stringify(body) : '<!doctype html><title>Synthetic teardown</title>');
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  if (!address || typeof address === 'string') throw Error('No loopback address');
  const origin = `http://127.0.0.1:${address.port}`;
  return {
    fixture,
    subject,
    requests,
    origin,
    received,
    setAnonymous() {
      anonymous = true;
    },
    rows(): Record<string, unknown>[] {
      try {
        return readFileSync(join(fixture.env.CMA_E2E_OUTPUT_DIR, 'cleanup.ndjson'), 'utf8')
          .trim()
          .split('\n')
          .map((line) => JSON.parse(line) as Record<string, unknown>);
      } catch {
        return [];
      }
    },
    harness(browser: Browser) {
      const previous = Object.fromEntries(
        Object.keys(fixture.env).map((key) => [key, process.env[key]]),
      );
      try {
        Object.assign(process.env, fixture.env);
        const h = new Harness(browser, {} as Hooks, 'J4-oauth');
        h.config.baseURL = origin;
        return h;
      } finally {
        for (const [key, value] of Object.entries(previous)) {
          if (value === undefined) delete process.env[key];
          else process.env[key] = value;
        }
      }
    },
    async own(page: Page, h: Harness) {
      await page.goto(origin);
      await page.evaluate(async () => {
        await (await fetch('/api/v1/triggers', { method: 'POST', body: '{}' })).text();
      });
      await h.flush();
    },
    async hold(page: Page, path = '/api/v1/held', method = 'GET') {
      const response =
        path === '/api/v1/held-no-headers'
          ? undefined
          : page.waitForResponse((r) => new URL(r.url()).pathname === path);
      await page.evaluate(
        ({ path, method }) => {
          void fetch(path, { method }).catch(() => {});
        },
        { path, method },
      );
      await received;
      await response;
    },
    release(body = ']') {
      pending?.end(body);
    },
    async close() {
      pending?.end();
      server.closeAllConnections();
      await new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      );
      rmSync(fixture.directory, { recursive: true, force: true });
    },
  };
}
