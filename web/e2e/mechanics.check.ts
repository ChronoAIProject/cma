import { test, expect, type Browser, type BrowserContext } from '@playwright/test';
import { mkdirSync, rmSync } from 'node:fs';
import { createServer, type RequestListener } from 'node:http';
import { workspaceAgentButton } from './lifecycle-creation';
import { assertPageSafe, readOnlyGuard } from './security';
import { inspectOwnerStream, ownerStreamTap, streamBounds } from './owner-stream';
import { Harness } from './harness';
import { fixtureDocument } from './environment.fixture';
import type { Hooks } from './environment';

// This suite tests the harness against intercepted local HTML, never staging journeys.
test('LOCAL smoke guard allows reads and refuses browser POST and DELETE', async ({ page }) => {
  const blocked: string[] = [];
  const served: string[] = [];
  await page.context().route('**/*', async (route) => {
    served.push(route.request().method());
    await route.fulfill({
      contentType: 'text/html',
      body: '<!doctype html><title>Local guard check</title><p>Public state</p>',
    });
  });
  await readOnlyGuard(page.context(), blocked);
  await page.goto('https://example.test/t/local-fixture');
  for (const method of ['POST', 'DELETE']) {
    const rejected = await page.evaluate(async (method) => {
      try {
        await fetch('/api/v1/sandboxes', { method });
        return false;
      } catch {
        return true;
      }
    }, method);
    expect(rejected).toBe(true);
  }
  expect(blocked).toEqual(['POST', 'DELETE']);
  expect(served.every((method) => method === 'GET')).toBe(true);
});

test('LOCAL credential checks reject storage, DOM and identity-provider pages before capture', async ({
  page,
}) => {
  await page.context().route('**/*', (route) =>
    route.fulfill({
      contentType: 'text/html',
      body: '<!doctype html><title>Local capture check</title><p>Public state</p>',
    }),
  );
  await page.goto('https://example.test/agents/local-agent');
  await expect(
    assertPageSafe(page, 'https://example.test', ['redacted-probe']),
  ).resolves.toBeUndefined();
  await page.evaluate(() => sessionStorage.setItem('unsafe', 'redacted-probe'));
  await expect(assertPageSafe(page, 'https://example.test', ['redacted-probe'])).rejects.toThrow(
    'Credential material',
  );
  await page.evaluate(() => {
    sessionStorage.clear();
    document.body.innerHTML = '<input type="password">';
  });
  await expect(assertPageSafe(page, 'https://example.test', [])).rejects.toThrow(
    'Credential-entry',
  );
  await page.goto('https://identity.example.test/register');
  await expect(assertPageSafe(page, 'https://example.test', [])).rejects.toThrow('canonical CMA');
});

test('LOCAL J6 selects the admitted Workspace among duplicate names and overlapping paths', async ({
  page,
}) => {
  await page.setContent(`
    <!doctype html><title>Local Workspace selection</title>
    <table class="sandbox-table"><tbody>
      <tr>
        <th>Shared profile</th><td><code>/workspaces/lifecycle-existing</code></td>
        <td><button aria-label="New agent in Shared profile" data-workspace="wks_existing">New agent here</button></td>
      </tr>
      <tr>
        <th>Shared profile</th><td><code>/workspaces/lifecycle</code></td>
        <td><button aria-label="New agent in Shared profile" data-workspace="wks_lifecycle">New agent here</button></td>
      </tr>
      <tr>
        <th>Shared profile</th><td><code>/workspaces/lifecycle/nested</code></td>
        <td><button aria-label="New agent in Shared profile" data-workspace="wks_nested">New agent here</button></td>
      </tr>
    </tbody></table>
    <output aria-label="Selected Workspace"></output>
  `);
  await page.evaluate(() => {
    for (const button of document.querySelectorAll<HTMLButtonElement>('button')) {
      button.addEventListener('click', () => {
        document.querySelector('output')!.textContent = button.dataset.workspace!;
      });
    }
  });
  const workspace = { id: 'wks_lifecycle', path: '/workspaces/lifecycle' };
  const button = workspaceAgentButton(page, workspace);
  await expect(
    page.getByRole('button', { name: 'New agent in Shared profile', exact: true }),
  ).toHaveCount(3);
  await expect(button).toHaveCount(1);
  await button.click();
  await expect(page.getByLabel('Selected Workspace')).toHaveText(workspace.id);
  await page.evaluate(() => {
    document
      .querySelector('tbody')!
      .append(document.querySelector('button[data-workspace="wks_existing"]')!.closest('tr')!);
    document.querySelector('output')!.textContent = '';
  });
  await button.click();
  await expect(page.getByLabel('Selected Workspace')).toHaveText(workspace.id);
  await expect(
    workspaceAgentButton(page, { id: 'wks_missing', path: '/workspaces/missing' }),
  ).toHaveCount(0);
});

for (const mode of ['complete', 'deadline'] as const) {
  test(`LOCAL owner stream tap ${mode} preserves one submission and bounds response observation`, async ({
    page,
  }) => {
    const frame = (type: string, fields = {}) =>
      `data: ${JSON.stringify({ type, threadId: 'agt_local', runId: '9', ...fields })}\n\n`;
    let commands = 0;
    let closed = false;
    let headersObserved = false;
    let chunksObserved = 0;
    const server = createServer((incoming, response) => {
      incoming.resume();
      if (incoming.url === '/api/v1/agui/run' && incoming.method === 'POST') {
        commands++;
        response.on('close', () => {
          closed = true;
        });
        response.writeHead(200, { 'Content-Type': 'text/event-stream' });
        response.write(frame('RUN_STARTED'));
        if (mode === 'complete')
          response.end(
            frame('TEXT_MESSAGE_CONTENT', { delta: 'Local second answer' }) +
              frame('RUN_FINISHED', { outcome: { type: 'complete' } }),
          );
      } else {
        response.writeHead(200, { 'Content-Type': 'text/html' });
        response.end('<!doctype html><title>Local owner stream</title>');
      }
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    const address = server.address();
    if (address === null || typeof address === 'string')
      throw new Error('Local stream port missing');
    const origin = `http://127.0.0.1:${address.port}`;
    const channel = new TransformStream<Uint8Array, Uint8Array>();
    const writer = channel.writable.getWriter();
    try {
      if (mode === 'deadline') {
        const time = new Date('2026-01-01T00:00:00Z');
        await page.clock.install({ time });
        await page.clock.pauseAt(time);
      }
      await page.route('**/*', (route) =>
        new URL(route.request().url()).origin === origin ? route.continue() : route.abort(),
      );
      await page.exposeBinding('__cmaOwnerStream', async (_source, event) => {
        expect(event.runId).toBe('local-correlation');
        if (event.stage === 'headers') {
          expect(event.status).toBe(200);
          expect(event.media).toBe('text/event-stream');
          headersObserved = true;
        } else if (event.stage === 'chunk') {
          await writer.write(Uint8Array.from(event.bytes));
          chunksObserved++;
        } else if (event.stage === 'end') await writer.close();
        else await writer.abort(new Error('Local observation cancelled'));
      });
      await page.addInitScript(ownerStreamTap, {
        origin,
        maxBytes: streamBounds.maxBytes,
        timeoutMs: mode === 'complete' ? 5000 : 200,
      });
      await page.goto(`${origin}/agents/agt_local`);
      const inspected = inspectOwnerStream(
        new Response(channel.readable),
        'agt_local',
        [],
        AbortSignal.timeout(5000),
      ).then(
        (turn) => ({ passed: true, turn }),
        () => ({ passed: false, turn: undefined }),
      );
      const consumed = page.evaluate(async () => {
        try {
          const response = await fetch('/api/v1/agui/run', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
              threadId: 'agt_local',
              runId: 'local-correlation',
              messages: [{ role: 'user', content: 'Local continuation' }],
            }),
          });
          await response.text();
          return true;
        } catch {
          return false;
        }
      });
      if (mode === 'deadline') {
        // Expire an admitted response, independently of browser/network setup speed.
        await expect.poll(() => commands).toBe(1);
        await expect.poll(() => headersObserved && chunksObserved > 0).toBe(true);
        await page.clock.runFor(200);
      }
      expect(await consumed).toBe(mode === 'complete');
      expect(await inspected).toEqual({
        passed: mode === 'complete',
        turn: mode === 'complete' ? 9 : undefined,
      });
      await expect.poll(() => closed).toBe(true);
      expect(commands).toBe(1);
    } finally {
      await writer.abort().catch(() => undefined);
      server.closeAllConnections();
      await new Promise<void>((resolve, reject) =>
        server.close((error) => (error ? reject(error) : resolve())),
      );
    }
  });
}

for (const attemptedWrite of [false, true]) {
  test(`LOCAL TC117 navigation ${attemptedWrite ? 'refuses automatic writes' : 'has no work effects'}`, async ({
    browser,
  }) => {
    const { mkdtempSync, writeFileSync } = await import('node:fs');
    const { tmpdir } = await import('node:os');
    const { join } = await import('node:path');
    const { navigationProbes } = await import('./visitor-abuse-load');
    const { newCounts } = await import('./visitor-abuse-model');
    const directory = mkdtempSync(join(tmpdir(), 'cma-tc117-navigation-'));
    const bearerFile = join(directory, 'bearer');
    writeFileSync(bearerFile, 'local-navigation-fixture', { mode: 0o600 });
    let writes = 0;
    let authorized = false;
    const service = createServer((incoming, response) => {
      if (incoming.method === 'POST') writes++;
      if (incoming.headers.authorization === 'Bearer local-navigation-fixture') authorized = true;
      incoming.resume();
      response.writeHead(200, { 'Content-Type': 'text/html' });
      response.end(
        `<!doctype html><title>Local TC117 navigation</title>${attemptedWrite ? '<script>fetch("/api/v1/triggers/public/local/launches", { method: "POST" }).catch(() => {});</script>' : ''}`,
      );
    });
    await new Promise<void>((resolve) => service.listen(0, '127.0.0.1', resolve));
    const address = service.address();
    if (!address || typeof address === 'string') throw new Error('Local port missing');
    const snapshot = {
      fleetLive: 0,
      launches: { b: [], c: [] },
      sandboxes: [],
      liveByVisitor: { b: 0, c: 0 },
      totals: {},
    };
    const driver = {
      config: { baseURL: `http://127.0.0.1:${address.port}` },
      fixture: {
        accounts: { c: { bearerFile } },
        triggers: { reuseA: { publicId: 'local', placementId: 'local-place' } },
      },
      counts: newCounts(),
      observations: {},
      snapshot: async () => snapshot,
      send: async () => ({ status: 200 }),
      read: async () => ({}),
    };
    try {
      const result = navigationProbes(
        driver as unknown as import('./visitor-abuse-client').TC117Client,
        browser,
      );
      if (attemptedWrite) await expect(result).rejects.toThrow('attempted a browser write');
      else await expect(result).resolves.toBeUndefined();
      expect(authorized).toBe(true);
      expect(writes).toBe(0);
    } finally {
      service.closeAllConnections();
      await new Promise<void>((resolve) => service.close(() => resolve()));
    }
  });
}

for (const start of ['same', 'path', 'origin', 'query', 'fragment', 'invalidated'] as const) {
  test(`LOCAL ready preserves response inspection with ${start} destination`, async ({
    browser,
  }) => {
    const fixture = fixtureDocument();
    mkdirSync(fixture.env.CMA_E2E_OUTPUT_DIR);
    const subject = fixture.document.visitors['J4-oauth']!.subject;
    const launch = {
      id: 'local-launch',
      state: 'ready',
      resolution: { agent: { id: 'local-agent' } },
    };
    const catalog = [{ id: 'local-catalog-entry' }];
    let launchReads = 0;
    const serve: RequestListener = (request, response) => {
      if (request.url === '/api/v1/launches/local-launch') launchReads++;
      const json =
        request.url === '/auth/session'
          ? { authenticated: true, subject }
          : request.url === '/api/v1/profile-authoring/services'
            ? catalog
            : request.url === '/api/v1/launches/local-launch'
              ? launch
              : undefined;
      response.writeHead(200, { 'Content-Type': json ? 'application/json' : 'text/html' });
      response.end(
        json
          ? JSON.stringify(json)
          : '<!doctype html><title>Local readiness</title><div id="agent-message">Ready</div>',
      );
    };
    const servers = [createServer(serve), createServer(serve)];
    const origins: string[] = [];
    let release!: () => void;
    const barrier = new Promise<void>((resolve) => {
      release = resolve;
    });
    let inspecting!: () => void;
    const inspectionStarted = new Promise<void>((resolve) => {
      inspecting = resolve;
    });
    let context: BrowserContext | undefined;
    let inspectedBody: string | undefined;
    let inspections = 0;
    const wrapped = {
      newContext: async (...args: Parameters<Browser['newContext']>) => {
        context = await browser.newContext(...args);
        context.on('response', (response) => {
          if (!response.url().endsWith('/api/v1/profile-authoring/services')) return;
          const original = response.text.bind(response);
          response.text = async () => {
            inspections++;
            inspecting();
            await barrier;
            inspectedBody = await original();
            return inspectedBody;
          };
        });
        return context;
      },
    } as Browser;
    try {
      for (const server of servers) {
        await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
        const address = server.address();
        if (!address || typeof address === 'string') throw new Error('Local port missing');
        origins.push(`http://127.0.0.1:${address.port}`);
      }
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
      h.config.baseURL = origins[0]!;
      const page = await h.open('visitor');
      const destination = `${h.config.baseURL}/agents/local-agent`;
      await page.goto(destination);
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
      expect(inspections).toBe(1);
      if (start === 'invalidated') await page.reload();
      else if (start !== 'same') {
        release();
        await h.flush();
        const initial = {
          path: `${h.config.baseURL}/before`,
          origin: `${origins[1]}/agents/local-agent`,
          query: `${destination}?view=local`,
          fragment: `${destination}#local`,
        }[start];
        await page.goto(initial);
      }
      let navigations = 0;
      page.on('framenavigated', (frame) => {
        if (frame === page.mainFrame()) navigations++;
      });
      expect(await h.ready(page, launch.id)).toEqual(launch);
      release();
      await h.flush();
      expect(h.violations).toEqual(start === 'invalidated' ? ['response_inspection_failed'] : []);
      expect(inspectedBody).toBe(start === 'invalidated' ? undefined : JSON.stringify(catalog));
      expect(navigations).toBe(start === 'same' || start === 'invalidated' ? 0 : 1);
      expect(launchReads).toBe(2);
      expect(page.url()).toBe(destination);
      expect(request.failure()).toBeNull();
      expect(h.commands).toEqual([]);
      expect(h.journal.resources).toEqual([]);
    } finally {
      release();
      await context?.close();
      for (const server of servers) {
        server.closeAllConnections();
        if (server.listening)
          await new Promise<void>((resolve, reject) =>
            server.close((error) => (error ? reject(error) : resolve())),
          );
      }
      rmSync(fixture.directory, { recursive: true, force: true });
    }
  });
}
