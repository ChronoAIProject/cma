import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { createServer } from 'node:http';
import { mkdirSync, rmSync } from 'node:fs';
import { test, expect } from '@playwright/test';
import {
  assertConnectionReplay,
  connectionCheckPath,
  connectionStartPath,
  connectionStarts,
  identityDigest,
  type ConnectionObservationInput,
} from './connection-journey-contract';
import { assertConnectionProof } from '../../.github/scripts/connection-journey-evidence.mjs';
import { connectionProof } from '../../.github/scripts/connection-journey-evidence.fixture.mjs';
import { Harness, type Command, type Launch } from './harness';
import { ConnectionJourney } from './connection-journey';
import { fixtureDocument } from './environment.fixture';
import type { Hooks } from './environment';
import { recorded } from './agent-startup-fixtures';

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

for (const scenario of [
  'valid',
  'explicit-self',
  'encoded-launch',
  'pending-callback',
  'missing-link',
  'hidden-link',
  'wrong-launch',
  'external-link',
  'recursive-link',
  'query-destination',
  'new-tab',
  'wrong-origin',
  'wrong-path',
  'no-verification',
  'wrong-owner',
  'changed-intent',
  'wrong-target',
  'missing-active',
  'wrong-service',
  'missing-checked-at',
  'uninitialized',
  'unacknowledged',
] as const) {
  test(`LOCAL J4 same-tab callback ${scenario}`, async ({ page, browser, context }, info) => {
    const fixture = fixtureDocument();
    mkdirSync(fixture.env.CMA_E2E_OUTPUT_DIR);
    const launch = {
      ...recorded<Launch>('launches.json', 'launch-review-create'),
      id: scenario === 'encoded-launch' ? 'launch/with space?' : 'launch-callback',
      state: 'draft',
    };
    const target = { type: 'launch' as const, id: launch.id };
    const service = fixture.document.connectionJourneys.services.oauth!;
    const owner = fixture.document.visitors['FAULT-popup-blocked']!.subject;
    const active = {
      service_id: service.serviceId,
      intent_id: '13572468-2468-1357-2468-135724681357',
    };
    const launchPath = `/launches/${encodeURIComponent(launch.id)}`;
    const destinations: Partial<Record<typeof scenario, string>> = {
      'wrong-launch': '/launches/another-launch',
      'external-link': 'https://identity.example.test/launches/launch-callback',
      'recursive-link': '/connection-return',
      'query-destination': `${launchPath}?return_to=/launches/another-launch`,
    };
    const callback = `<!doctype html><title>Local callback control</title>${
      scenario === 'missing-link'
        ? ''
        : `<a href="${destinations[scenario] ?? launchPath}"
           ${scenario === 'hidden-link' ? 'hidden' : ''}
           ${scenario === 'new-tab' ? 'target="_blank"' : scenario === 'explicit-self' ? 'target="_self"' : ''}
           onclick="sessionStorage.setItem('callback-clicks', '1')">Return to CMA</a>`
    }`;
    const check = {
      target: scenario === 'wrong-target' ? { type: 'launch', id: 'another-launch' } : target,
      ...(scenario === 'missing-active'
        ? {}
        : {
            active: {
              ...active,
              intent_id: scenario === 'changed-intent' ? 'another-intent' : active.intent_id,
            },
          }),
    };
    const visits: string[] = [];
    const checks: unknown[] = [];
    const mutations: string[] = [];
    const server = createServer(async (request, response) => {
      const path = new URL(request.url!, 'http://127.0.0.1').pathname;
      if (request.method === 'POST') {
        mutations.push(path);
        const chunks: Buffer[] = [];
        for await (const chunk of request) chunks.push(Buffer.from(chunk));
        if (path === connectionCheckPath) {
          checks.push(JSON.parse(Buffer.concat(chunks).toString()));
          response.writeHead(200, { 'Content-Type': 'application/json' });
          response.end(
            JSON.stringify({
              owner: scenario === 'wrong-owner' ? 'another-owner' : owner,
              ready: true,
              services: [
                {
                  service: scenario === 'wrong-service' ? 'another-service' : active.service_id,
                  state: 'satisfied',
                },
              ],
              ...(scenario === 'missing-checked-at'
                ? {}
                : { checked_at: new Date().toISOString() }),
            }),
          );
        } else response.writeHead(400).end();
        return;
      }
      response.writeHead(200, { 'Content-Type': 'text/html' });
      if (path.startsWith('/launches/')) {
        visits.push(path);
        response.end(
          `<!doctype html><title>Original Launch</title>${
            scenario === 'no-verification'
              ? ''
              : `<script>fetch('${connectionCheckPath}', { method: 'POST',
              headers: { 'Content-Type': 'application/json' }, body: ${JSON.stringify(JSON.stringify(check))} });</script>`
          }`,
        );
      } else if (path === '/') {
        response.end(
          '<!doctype html><section aria-label="Required connections">Local wizard</section>',
        );
      } else response.end(callback);
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    const address = server.address();
    assert.ok(address && typeof address !== 'string');
    const origin = `http://127.0.0.1:${address.port}`;
    const observeConnection = async (input: ConnectionObservationInput) => ({
      schema: 'cma-j4-provider-observation-v1',
      run: input.run,
      caseId: input.caseId,
      phase: input.phase,
      mode: input.mode,
      ownerDigest: identityDigest(owner),
      targetDigest: identityDigest(target),
      serviceDigest: identityDigest(active.service_id),
      intentDigest: null,
      providerRevision: service.providerRevision,
      fixtureDigest: service.fixtureDigest,
      observedAt: new Date().toISOString(),
      evidenceDigest: identityDigest('local-before-observation'),
      links: [],
    });
    const previous = Object.fromEntries(
      Object.keys(fixture.env).map((key) => [key, process.env[key]]),
    );
    try {
      Object.assign(process.env, fixture.env);
      const h = new Harness(browser, { observeConnection } as Hooks, 'FAULT-popup-blocked');
      h.config.baseURL = origin;
      page.setDefaultTimeout(1500);
      const journey = new ConnectionJourney(h, 'oauth');
      await page.goto(origin);
      if (scenario !== 'uninitialized') await journey.begin(page, launch);
      if (!['uninitialized', 'unacknowledged'].includes(scenario))
        journey.readStart({ target, ...active }, { owner, ...active });
      const callbackURL = `${origin}/connection-return?status=completed`;
      if (scenario === 'wrong-origin') {
        await page.route('http://other.example.test/**', (route) =>
          route.fulfill({ contentType: 'text/html', body: callback }),
        );
        await page.goto('http://other.example.test/connection-return?status=completed');
      } else if (scenario === 'wrong-path' || scenario === 'pending-callback')
        await page.goto(`${origin}/unrelated`);
      else await page.goto(callbackURL);
      const beforeURL = page.url();
      const tabs = context.pages().length;
      const successful = ['valid', 'explicit-self', 'encoded-launch', 'pending-callback'].includes(
        scenario,
      );
      const verifies = [
        'no-verification',
        'wrong-owner',
        'changed-intent',
        'wrong-target',
        'missing-active',
        'wrong-service',
        'missing-checked-at',
      ].includes(scenario);
      if (scenario === 'pending-callback') {
        await Promise.all([journey.returnFromSameTab(page), page.goto(callbackURL)]);
      } else if (successful) await journey.returnFromSameTab(page);
      else {
        const reasons: Partial<Record<typeof scenario, RegExp>> = {
          'missing-link': /toBeVisible/,
          'hidden-link': /toBeVisible/,
          'wrong-launch': /Callback must return to the original Launch/,
          'external-link': /Callback must return to the original Launch/,
          'recursive-link': /Callback must return to the original Launch/,
          'query-destination': /Callback must return to the original Launch/,
          'new-tab': /Callback must stay in the original tab/,
          'wrong-origin': /toHaveURL/,
          'wrong-path': /toHaveURL/,
          'no-verification': /Timeout.*exceeded/,
          'wrong-owner': /Connection verification owner changed/,
          'changed-intent': /Ready response did not verify the original intent/,
          'wrong-target': /Expected values to be strictly deep-equal/,
          'missing-active': /Timeout.*exceeded/,
          'wrong-service': /Required service disappeared/,
          'missing-checked-at': /report.checked_at/,
          uninitialized: /requires an acknowledged Launch intent/,
          unacknowledged: /requires an acknowledged Launch intent/,
        };
        assert.ok(reasons[scenario]);
        await assert.rejects(journey.returnFromSameTab(page), reasons[scenario]);
      }
      if (successful || verifies) {
        expect(page.url()).toBe(origin + launchPath);
        expect(await page.evaluate(() => sessionStorage.getItem('callback-clicks'))).toBe('1');
        expect(visits).toEqual([launchPath]);
        expect(checks).toEqual(scenario === 'no-verification' ? [] : [check]);
      } else {
        expect(page.url()).toBe(beforeURL);
        expect(await page.evaluate(() => sessionStorage.getItem('callback-clicks'))).toBeNull();
        expect(visits).toEqual([]);
        expect(checks).toEqual([]);
      }
      expect(context.pages()).toHaveLength(tabs);
      expect(mutations.every((path) => path === connectionCheckPath)).toBe(true);
      await h.attachJourneyProof(info);
      expect(info.attachments).toEqual([]);
    } finally {
      for (const [key, value] of Object.entries(previous)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
      }
      await page.close();
      await new Promise<void>((resolve, reject) => {
        server.close((error) => (error ? reject(error) : resolve()));
        server.closeAllConnections();
      });
      rmSync(fixture.directory, { recursive: true, force: true });
    }
  });
}
