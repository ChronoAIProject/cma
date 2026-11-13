import { expect, test } from '@playwright/test';
import { createServer } from 'node:http';
import { cursor, LocalAgentV2 } from './agent-v2.fixture';

for (const stalled of ['headers', 'body'])
  test(`LOCAL stalled ${stalled} preview preserves input and offers a bounded retry`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    let reads = 0;
    let cancelled = 0;
    const server = createServer((request, response) => {
      response.setHeader('Access-Control-Allow-Origin', baseURL!);
      response.setHeader('Access-Control-Allow-Credentials', 'true');
      response.setHeader('Content-Type', 'application/json');
      reads += 1;
      if (reads === 1) {
        response.on('close', () => {
          cancelled += 1;
        });
        if (stalled === 'body') response.write('{"path":');
        return;
      }
      const path = new URL(request.url!, baseURL).searchParams.get('path');
      response.end(
        JSON.stringify({
          path,
          size_bytes: 5,
          sha256: 'a'.repeat(64),
          content_base64: Buffer.from('Notes').toString('base64'),
        }),
      );
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    const address = server.address();
    if (!address || typeof address === 'string') throw new Error('No file fixture port');
    const origin = `http://127.0.0.1:${address.port}`;
    try {
      await page.setViewportSize({ width: 1600, height: 1000 });
      await fixture.route(page, baseURL!);
      await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
        route.fulfill({
          json: {
            object: 'cma.agent.event_head',
            schema_version: 1,
            agent_id: 'agt_contract',
            incarnation: 'a'.repeat(64),
            through_cursor: cursor(fixture.items.length),
            active_turn: null,
          },
        }),
      );
      await page.route('**/api/v1/agents/agt_contract/files/tree**', (route) => {
        const path = new URL(route.request().url()).searchParams.get('path')!;
        return route.fulfill({
          json: {
            path,
            entries: [
              {
                path: `${path ? `${path}/` : ''}notes.txt`,
                name: 'notes.txt',
                kind: 'file',
                size_bytes: 5,
                hidden: false,
              },
            ],
          },
        });
      });
      await page.route(`${origin}/**`, (route) => route.continue());
      await page.route('**/api/v1/agents/agt_contract/files/content**', (route) => {
        const url = new URL(route.request().url());
        return route.continue({ url: `${origin}${url.pathname}${url.search}` });
      });
      await page.goto('/agents/agt_contract');
      const message = page.getByRole('textbox', { name: 'Message', exact: true });
      await expect(message).toBeEnabled();
      const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
      await computer.getByRole('tab', { name: 'Files', exact: true }).click();
      await computer.getByRole('button', { name: 'notes.txt', exact: true }).click();
      await expect(computer.getByText('Opening file…')).toBeVisible();
      await message.fill('My draft remains usable during a stalled file read.');
      await expect(computer.getByRole('button', { name: 'Retry preview' })).toBeVisible({
        timeout: 6000,
      });
      expect(reads).toBe(1);
      await expect.poll(() => cancelled).toBe(1);
      await computer.getByRole('button', { name: 'Retry preview' }).click();
      await expect(computer.getByRole('textbox', { name: 'Contents of notes.txt' })).toHaveValue(
        'Notes',
      );
      await expect(message).toHaveValue('My draft remains usable during a stalled file read.');
      expect(reads).toBe(2);
      expect(fixture.writes).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      await page.screenshot({ path: info.outputPath('preview-after-retry.png') });
    } catch (error) {
      await info.attach('preview-dom.txt', {
        body: await page.locator('body').innerText(),
        contentType: 'text/plain',
      });
      throw error;
    } finally {
      await fixture.close(page);
      server.closeAllConnections();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  });
