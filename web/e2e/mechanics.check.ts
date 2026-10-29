import { test, expect } from '@playwright/test';
import { createServer } from 'node:http';
import { workspaceAgentButton } from './lifecycle-creation';
import { assertPageSafe, readOnlyGuard } from './security';
import { inspectOwnerStream, ownerStreamTap, streamBounds } from './owner-stream';

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
