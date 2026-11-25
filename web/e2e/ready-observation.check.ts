import { createServer } from 'node:http';
import { mkdirSync, rmSync } from 'node:fs';
import { test, expect, type Browser, type BrowserContext } from '@playwright/test';
import { Harness } from './harness';
import { fixtureDocument } from './environment.fixture';
import type { Hooks } from './environment';

for (const mode of ['same', 'required-held', 'required-drained', 'arrived-during-drain'] as const) {
  const alreadyAtAgent = mode === 'same';
  test(`LOCAL ready drains existing observations for ${mode}`, async ({ browser }) => {
    const fixture = fixtureDocument();
    mkdirSync(fixture.env.CMA_E2E_OUTPUT_DIR);
    const subject = fixture.document.visitors['J4-oauth']!.subject;
    let launchReads = 0;
    const server = createServer((request, response) => {
      if (request.url === '/api/v1/launches/synthetic-launch') launchReads++;
      const json =
        request.url === '/auth/session'
          ? { authenticated: true, subject }
          : request.url === '/api/v1/profile-authoring/services'
            ? [{ id: 'synthetic-catalog-entry' }]
            : request.url === '/api/v1/launches/synthetic-launch'
              ? {
                  id: 'synthetic-launch',
                  state: 'ready',
                  resolution: { agent: { id: 'synthetic-agent' } },
                }
              : undefined;
      if (json !== undefined) {
        response.writeHead(200, { 'Content-Type': 'application/json' });
        response.end(JSON.stringify(json));
      } else {
        response.writeHead(200, { 'Content-Type': 'text/html' });
        response.end(
          '<!doctype html><title>Synthetic readiness</title><div id="agent-message">Ready</div>',
        );
      }
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    const address = server.address();
    if (address === null || typeof address === 'string') throw Error('No local address');
    let release!: () => void;
    const barrier = new Promise<void>((resolve) => {
      release = resolve;
    });
    let inspecting!: () => void;
    const inspectionStarted = new Promise<void>((resolve) => {
      inspecting = resolve;
    });
    let context: BrowserContext | undefined;
    let intercepted = 0;
    let inspectedBody: string | undefined;
    const wrapped = {
      newContext: async (...args: Parameters<Browser['newContext']>) => {
        context = await browser.newContext(...args);
        context.on('response', (response) => {
          if (!response.url().endsWith('/api/v1/profile-authoring/services')) return;
          const original = response.text.bind(response);
          response.text = async () => {
            intercepted += 1;
            inspecting();
            await barrier;
            inspectedBody = await original();
            return inspectedBody;
          };
        });
        return context;
      },
    } as Browser;
    const previous = Object.fromEntries(
      Object.keys(fixture.env).map((key) => [key, process.env[key]]),
    );
    let h: Harness;
    try {
      Object.assign(process.env, fixture.env);
      h = new Harness(wrapped, {} as Hooks, 'J4-oauth');
    } finally {
      for (const [key, value] of Object.entries(previous)) {
        if (value === undefined) delete process.env[key];
        else process.env[key] = value;
      }
    }
    h.config.baseURL = `http://127.0.0.1:${address.port}`;
    try {
      const page = await h.open('visitor');
      const agentUrl = h.config.baseURL + '/agents/synthetic-agent';
      await page.goto(alreadyAtAgent ? agentUrl : h.config.baseURL + '/before');
      const finished = page.waitForEvent('requestfinished', (request) =>
        request.url().endsWith('/api/v1/profile-authoring/services'),
      );
      expect(
        await page.evaluate(async () => {
          const response = await fetch('/api/v1/profile-authoring/services');
          await response.text();
          return response.status;
        }),
      ).toBe(200);
      const request = await finished;
      await inspectionStarted;
      expect(request.failure()).toBeNull();
      expect(intercepted).toBe(1);
      let navigations = 0;
      page.on('load', () => {
        navigations += 1;
      });
      if (mode === 'required-drained') {
        release();
        await h.flush();
      }
      let signalFlush!: () => void;
      const flushing = new Promise<void>((resolve) => {
        signalFlush = resolve;
      });
      let flushCalls = 0;
      const originalFlush = h.flush.bind(h);
      h.flush = async () => {
        flushCalls++;
        signalFlush();
        await originalFlush();
      };
      let signalNavigation!: () => void;
      const navigation = new Promise<void>((resolve) => {
        signalNavigation = resolve;
      });
      page.on('framenavigated', (frame) => {
        if (frame === page.mainFrame()) signalNavigation();
      });
      const readiness = h.ready(page, 'synthetic-launch');
      const first = await Promise.race([
        flushing.then(() => 'flush' as const),
        navigation.then(() => 'navigation' as const),
        readiness.then(() => 'ready' as const),
      ]);
      if (mode === 'arrived-during-drain') {
        expect(first).toBe('flush');
        expect(page.url()).toBe(h.config.baseURL + '/before');
        await page.evaluate((url) => history.pushState({}, '', url), agentUrl);
      }
      release();
      const launch = await readiness;
      const readinessFlushes = flushCalls;
      await h.flush();
      expect(request.failure()).toBeNull();
      expect(launch.id).toBe('synthetic-launch');
      expect(page.url()).toBe(agentUrl);
      expect(h.violations).toEqual([]);
      expect(inspectedBody).toBe(JSON.stringify([{ id: 'synthetic-catalog-entry' }]));
      expect(first).toBe(alreadyAtAgent ? 'ready' : 'flush');
      expect(readinessFlushes).toBe(alreadyAtAgent ? 0 : 1);
      expect(navigations).toBe(alreadyAtAgent || mode === 'arrived-during-drain' ? 0 : 1);
      expect(h.journal.resources).toEqual([]);
      expect(h.commands).toEqual([]);
      expect(launchReads).toBe(2);
    } finally {
      release();
      await context?.close();
      server.closeAllConnections();
      await new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      );
      rmSync(fixture.directory, { recursive: true, force: true });
    }
  });
}
