import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { test, expect } from '@playwright/test';
import {
  assertConnectionReplay,
  connectionStartPath,
  connectionStarts,
  identityDigest,
} from './connection-journey-contract';
import { assertConnectionProof } from '../../.github/scripts/connection-journey-evidence.mjs';
import { connectionProof } from '../../.github/scripts/connection-journey-evidence.fixture.mjs';
import type { Command } from './harness';

for (const scenario of ['stable-deduplicated', 'stable-duplicated', 'changed-intent'] as const) {
  test(`LOCAL J4 response-loss boundary distinguishes ${scenario}`, async ({ page, context }) => {
    const links: { intent: string; digest: string }[] = [];
    const requests: string[] = [];
    const server = createServer(async (request, response) => {
      if (request.url === '/') {
        response.writeHead(200, { 'Content-Type': 'text/html' });
        response.end('<!doctype html><title>Local connection transport fixture</title>');
        return;
      }
      const chunks: Buffer[] = [];
      for await (const chunk of request) chunks.push(Buffer.from(chunk));
      const body = JSON.parse(Buffer.concat(chunks).toString() || '{}');
      response.setHeader('Content-Type', 'application/json');
      if (request.url === connectionStartPath) {
        requests.push(body.intent_id);
        let link =
          scenario === 'stable-duplicated'
            ? undefined
            : links.find((row) => row.intent === body.intent_id);
        if (!link) {
          link = {
            intent: body.intent_id,
            digest: identityDigest(`local-link-${links.length + 1}`),
          };
          links.push(link);
        }
        response.end(
          JSON.stringify({
            owner: 'local-owner',
            service_id: 'sample-service',
            intent_id: body.intent_id,
            hosted_url: 'https://identity.example.test/connect',
            expires_at: '2099-01-01T00:00:00Z',
          }),
        );
      } else response.end('{}');
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    const address = server.address();
    if (address === null || typeof address === 'string')
      throw new Error('Local server has no port');
    const origin = `http://127.0.0.1:${address.port}`;
    const commands: Command[] = [];
    context.on('request', (request) => {
      const url = new URL(request.url());
      if (url.origin === origin && request.method() === 'POST')
        commands.push({
          method: 'POST',
          path: url.pathname,
          bodyHash: createHash('sha256')
            .update(request.postData() ?? '')
            .digest('hex'),
        });
    });
    const post = (path: string, body: unknown) =>
      page.evaluate(
        async ({ path, body }) => {
          try {
            const response = await fetch(path, {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify(body),
            });
            return response.ok ? 'received' : 'refused';
          } catch {
            return 'lost';
          }
        },
        { path, body },
      );
    try {
      await page.goto(origin);
      await post('/api/v1/triggers/public/trg_local/launches', {});
      const active = {
        service_id: 'sample-service',
        intent_id: '13572468-2468-1357-2468-135724681357',
      };
      const target = { type: 'launch', id: 'lch_local' };
      await post('/auth/connections/check', { target });
      let dropped = false;
      await page.route(`**${connectionStartPath}`, async (route) => {
        if (dropped) return route.fallback();
        const accepted = await route.fetch();
        expect(accepted.ok()).toBe(true);
        expect(links).toHaveLength(1);
        dropped = true;
        await route.abort('connectionfailed');
      });
      expect(await post(connectionStartPath, { target, ...active, force: false })).toBe('lost');
      expect(dropped).toBe(true);
      expect(requests).toEqual([active.intent_id]);
      const before = [...commands];
      const retry = {
        ...active,
        intent_id:
          scenario === 'changed-intent' ? '24681357-1357-2468-1357-246813572468' : active.intent_id,
      };
      expect(await post(connectionStartPath, { target, ...retry, force: false })).toBe('received');
      expect(connectionStarts(commands)).toHaveLength(2);
      expect(requests).toHaveLength(2);
      if (scenario === 'changed-intent') {
        expect(() => assertConnectionReplay(before, commands)).toThrow('original command');
        expect(links).toHaveLength(2);
      } else {
        expect(() => assertConnectionReplay(before, commands)).not.toThrow();
        await post('/auth/connections/check', { target, active });
        await post('/api/v1/launches/lch_local/start', {});
        const proof = connectionProof('FAULT-handoff-response');
        proof.commands = commands;
        proof.intentDigest = identityDigest(active.intent_id);
        proof.after.intentDigest = proof.intentDigest;
        proof.after.links = links.map((link) => ({ linkDigest: link.digest, state: 'completed' }));
        const validate = () =>
          assertConnectionProof(proof, {
            run: proof.run,
            caseId: proof.caseId,
            providerRevision: proof.providerRevision,
          });
        if (scenario === 'stable-duplicated') {
          expect(links).toHaveLength(2);
          expect(validate).toThrow('Exactly one new provider link');
        } else {
          expect(links).toHaveLength(1);
          expect(validate).not.toThrow();
        }
      }
    } finally {
      await page.close();
      await new Promise<void>((resolve, reject) => {
        server.close((error) => (error ? reject(error) : resolve()));
        server.closeAllConnections();
      });
    }
  });
}
