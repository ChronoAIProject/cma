import { readFileSync } from 'node:fs';
import { mkdir } from 'node:fs/promises';
import { createServer, type ServerResponse } from 'node:http';
import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

const proofPath = process.env.CMA_PLAN_PUBLIC_PROOF;
if (!proofPath) throw new Error('Set CMA_PLAN_PUBLIC_PROOF to the public Rust route proof output.');
const proof = JSON.parse(readFileSync(proofPath, 'utf8')) as {
  initial: string;
  changed: string;
  replay: string;
};
const evidence = process.env.CMA_PLAN_EVIDENCE_DIR ?? '/tmp/cma-ticket06-browser';

function tail(wire: string, after: number) {
  let pending = '';
  let output = '';
  for (const block of wire.split('\n\n')) {
    if (block === '') continue;
    pending += `${block}\n\n`;
    const sequence = /^id: (\d+)$/m.exec(block)?.[1];
    if (sequence === undefined) continue;
    if (Number(sequence) > after) output += pending;
    pending = '';
  }
  return output;
}

for (const width of [1440, 390]) {
  test(`LOCAL pinned plan public frames survive change and reconnect at ${width}px`, async ({
    page,
    baseURL,
  }) => {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 1000 });
    await page.emulateMedia({
      reducedMotion: 'reduce',
      colorScheme: width === 390 ? 'dark' : 'light',
    });
    const boundary = new LocalStartup();
    let stream: ServerResponse | undefined;
    let current = proof.initial;
    let connections = 0;
    const server = createServer((request, response) => {
      response.writeHead(200, {
        'Content-Type': 'text/event-stream',
        'Access-Control-Allow-Origin': baseURL!,
        'Access-Control-Allow-Credentials': 'true',
        'Access-Control-Allow-Headers': 'content-type, x-request-id, last-event-id',
        'Access-Control-Allow-Methods': 'GET, OPTIONS',
      });
      if (request.method === 'OPTIONS') return response.end();
      connections++;
      stream = response;
      const after = Number(
        new URL(request.url!, streamURL).searchParams.get('starting_after') ?? 0,
      );
      response.write(tail(current, after) || ': connected\n\n');
    });
    await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
    const address = server.address();
    if (address === null || typeof address === 'string') throw new Error('Missing stream address');
    const streamURL = `http://127.0.0.1:${address.port}/events`;
    try {
      await boundary.route(page, baseURL!);
      await page.route(streamURL, (route) => route.continue());
      await page.route('**/api/v1/agui/events*', (route) =>
        route.continue({ url: `${streamURL}${new URL(route.request().url()).search}` }),
      );
      await page.goto('/agents/agt_contract');
      const plan = page.getByRole('region', { name: 'Plan', exact: true });
      await expect(plan).toHaveCount(1);
      await mkdir(evidence, { recursive: true });
      await page.screenshot({ path: `${evidence}/initial-${width}.png`, fullPage: true });
      await expect(plan.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '0');
      const scroller = page.locator('.agent-surface__conversation');
      if (width === 1440) {
        stream!.write(
          `data: ${JSON.stringify({
            type: 'TEXT_MESSAGE_CONTENT',
            threadId: 'agt_contract',
            runId: '3',
            messageId: 'browser-scroll-stress',
            'cma:live': true,
            delta: 'A long browser fixture tests reading earlier conversation content.\n\n'.repeat(
              100,
            ),
          })}\n\n`,
        );
        await expect
          .poll(() => scroller.evaluate((element) => element.scrollHeight > element.clientHeight))
          .toBe(true);
        await scroller.hover();
        await page.mouse.wheel(0, -100000);
        await expect.poll(() => scroller.evaluate((element) => element.scrollTop)).toBe(0);
      }
      current = proof.changed;
      stream!.write(proof.changed.slice(proof.initial.length));
      await expect(
        plan.getByRole('progressbar', { name: 'Reported plan progress' }),
      ).toHaveAttribute('aria-valuenow', '1');
      if (width === 1440) {
        expect(await scroller.evaluate((element) => element.scrollTop)).toBe(0);
        await page.reload();
        await expect(plan.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '1');
      }
      const bar = plan.getByRole('progressbar');
      const filled = await bar.locator('span').boundingBox();
      expect(filled!.width / (await bar.boundingBox())!.width).toBeCloseTo(1 / 3, 2);
      expect(filled!.height).toBeGreaterThan(0);
      await expect(bar.locator('span')).not.toHaveCSS('background-color', 'rgba(0, 0, 0, 0)');
      await expect(plan).toContainText('Current step 2: Implement the change');
      const header = page.getByRole('link', { name: 'Current plan: 1/3 steps · Step 2' });
      await expect(header).toBeVisible();
      const draft = page.getByRole('textbox', { name: 'Message', exact: true });
      await draft.fill('Keep my next task draft');
      await expect(draft).toHaveValue('Keep my next task draft');
      const beforeReconnect = connections;
      stream!.end();
      await expect.poll(() => connections).toBeGreaterThan(beforeReconnect);
      await expect(plan).toHaveCount(1);
      await expect(plan.getByRole('progressbar')).toHaveAttribute('aria-valuenow', '1');
      await expect(draft).toHaveValue('Keep my next task draft');
      await header.focus();
      await page.keyboard.press('Enter');
      await expect(plan).toBeFocused();
      const toggle = plan.getByRole('button', { name: 'View plan and operations' });
      await toggle.focus();
      await page.keyboard.press('Enter');
      await expect(toggle).toHaveAttribute('aria-expanded', 'false');
      await page.keyboard.press('Enter');
      await expect(toggle).toHaveAttribute('aria-expanded', 'true');
      if (width === 390) {
        expect((await toggle.boundingBox())!.height).toBeGreaterThanOrEqual(44);
        expect((await header.boundingBox())!.height).toBeGreaterThanOrEqual(44);
      }
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      );
      await page.screenshot({ path: `${evidence}/active-${width}.png`, fullPage: true });
      current = proof.replay;
      stream!.write(proof.replay.slice(proof.changed.length));
      await expect(plan).toContainText('Response finished · Plan completion not fully reported');
      await expect(header).toHaveCount(0);
      await page.reload();
      await expect(plan).toHaveCount(1);
      await expect(toggle).toHaveAttribute('aria-expanded', 'false');
      await expect(plan).toContainText('Completed 1/3 steps');
      await expect(plan.getByText(/Completed all|\bETA\b|estimated remaining/i)).toHaveCount(0);
      await toggle.click();
      await expect(plan.getByText('Waiting', { exact: true })).toBeVisible();
      await page.screenshot({ path: `${evidence}/replayed-${width}.png`, fullPage: true });
      expect(boundary.errors).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.writes).toEqual([]);
    } finally {
      server.closeAllConnections();
      await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  });
}
