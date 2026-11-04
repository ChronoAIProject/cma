import { expect, test } from '@playwright/test';
import assert from 'node:assert/strict';
import { createHash, randomUUID } from 'node:crypto';
import { readFile, rename, unlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import type { BrowserObservation } from '../../sdk/typescript/src/index';
import { LocalStartup, localTime } from './agent-startup-fixtures';

test('NATIVE Agent navigation reaches authorized public browser observations and the Web panel', async ({
  page,
  request,
  baseURL,
}, info) => {
  const directory = process.env.CMA_BROWSER_RELAY_DIRECTORY!;
  const origins = JSON.parse(await readFile(join(directory, 'public-ready.json'), 'utf8'));
  const root = '/api/v1/agents/agt_shared_read';
  const environment = await request.get(origins.owner + root + '/environment');
  expect(environment.status()).toBe(200);
  const initial = await environment.json();
  expect(initial.browser.state).toBe('not_enabled');
  expect(initial.control.state).toBe('unsupported');
  const incarnation = initial.runtime_incarnation;
  const receipts: BrowserObservation[] = [];
  let reads = 0;
  let releases = 0;
  let disconnected = false;
  const boundary = new LocalStartup();
  boundary.agent = { ...boundary.agent, id: 'agt_shared_read' };
  boundary.sandbox = { ...boundary.sandbox, owner: 'owner-a' };
  boundary.workspace = {
    ...boundary.workspace,
    agents: boundary.workspace.agents.map((agent) => ({ ...agent, id: 'agt_shared_read' })),
  };
  await boundary.route(page, baseURL!);
  await page.route('**/auth/session', (route) =>
    route.fulfill({
      json: {
        authenticated: true,
        subject: 'owner-a',
        display_name: 'Native proof owner',
        capabilities: ['use_own_sandboxes', 'create_sandbox', 'delete_own_sandbox'],
      },
    }),
  );
  await page.route('**/auth/connections/check', (route) =>
    route.fulfill({
      json: {
        owner: 'owner-a',
        ready: true,
        checked_at: localTime,
        services: [],
      },
    }),
  );
  await page.route(
    /\/api\/v1\/agents\/agt_shared_read\/(browser|environment)(\?|$)/,
    async (route) => {
      const outgoing = route.request();
      const url = new URL(outgoing.url());
      if (url.pathname.endsWith('/browser')) {
        if (outgoing.method() === 'GET') {
          reads += 1;
          if (disconnected) return route.abort();
        }
        if (outgoing.method() === 'DELETE') releases += 1;
      }
      const response = await route.fetch({ url: origins.owner + url.pathname + url.search });
      if (url.pathname.endsWith('/browser') && response.ok()) {
        expect(response.headers()['cache-control']).toBe('private, no-store');
        receipts.push(await response.json());
      }
      await route.fulfill({ response });
    },
  );
  const navigate = async (destination: 'a' | 'b') => {
    const identifier = randomUUID();
    const temporary = join(directory, identifier + '.tmp');
    await writeFile(temporary, JSON.stringify({ t: 'native_navigation', page: destination }));
    await rename(temporary, join(directory, identifier + '.request.json'));
    const response = join(directory, identifier + '.response.json');
    for (let attempt = 0; attempt < 500; attempt += 1) {
      const bytes = await readFile(response).catch(() => undefined);
      if (bytes) {
        await unlink(response);
        const receipt = JSON.parse(bytes.toString());
        expect(receipt.page).toBe(destination);
        expect(receipt.nativeToolCompletions).toBeGreaterThanOrEqual(destination === 'a' ? 1 : 2);
        return receipt;
      }
      await delay(40);
    }
    throw new Error('Actual native navigation did not complete');
  };
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/agents/agt_shared_read');
  await page.getByRole('textbox', { name: 'Message' }).fill('Preserve the browser conversation');
  const openComputer = page.getByRole('button', { name: 'Show Agent computer', exact: true });
  if (await openComputer.isVisible()) await openComputer.click();
  await page.getByRole('tab', { name: 'Browser', exact: true }).click();
  await page.getByRole('button', { name: 'Enable browser', exact: true }).click();
  await expect(page.getByText('Live', { exact: true })).toBeVisible({ timeout: 15000 });
  const image = page.getByAltText('Agent browser page');
  let previous = await image.getAttribute('src');
  const frames = [];
  for (const destination of ['a', 'b'] as const) {
    await navigate(destination);
    await expect.poll(() => image.getAttribute('src')).not.toBe(previous);
    await expect(image).toHaveJSProperty('naturalWidth', 1280);
    await expect(image).toHaveJSProperty('naturalHeight', 720);
    await expect(page.getByText('Live', { exact: true })).toBeVisible();
    previous = await image.getAttribute('src');
    assert(previous, 'Native navigation must display a browser image');
    const encoded = previous.split(',')[1];
    assert(encoded, 'Displayed browser image must contain base64 frame data');
    const bytes = Buffer.from(encoded, 'base64');
    expect(bytes.length).toBeLessThanOrEqual(262144);
    const receipt = receipts
      .slice()
      .reverse()
      .find((value) => value.frame?.data === encoded);
    assert(receipt?.frame, 'Displayed browser frame must have an authorized receipt');
    expect(receipt.agent_id).toBe('agt_shared_read');
    expect(receipt.runtime_incarnation).toBe(incarnation);
    expect(receipt.location).toBe('Local page');
    expect(receipt.context_id).toBeTruthy();
    frames.push({
      page: destination,
      sha256: createHash('sha256').update(bytes).digest('hex'),
      bytes: bytes.length,
      sequence: receipt.frame.sequence,
      captured_at: receipt.frame.captured_at,
      context_id: receipt.context_id,
      runtime_incarnation: incarnation,
    });
    await writeFile(join(directory, `public-page-${destination}.jpg`), bytes);
    await page.screenshot({
      path: info.outputPath(`native-page-${destination}.png`),
      fullPage: true,
    });
  }
  const [pageA, pageB] = frames;
  assert(pageA && pageB, 'Native navigation must produce evidence for both pages');
  expect(pageA.sha256).not.toBe(pageB.sha256);
  expect(pageA.context_id).toBe(pageB.context_id);
  expect(pageB.sequence).toBeGreaterThan(pageA.sequence);
  const query = `?runtime_incarnation=${incarnation}&observer=chain-api`;
  expect((await request.get(origins.foreign + root + '/browser' + query)).status()).toBe(404);
  expect(
    (
      await request.put(origins.viewer + root + '/browser', {
        data: { runtime_incarnation: incarnation, enabled: true },
      })
    ).status(),
  ).toBe(403);
  expect(
    (
      await request.get(
        origins.owner + root + '/browser?runtime_incarnation=stale&observer=chain-api',
      )
    ).status(),
  ).toBe(409);
  disconnected = true;
  await expect(page.getByText('Live', { exact: true })).not.toBeVisible({ timeout: 5000 });
  await expect(page.getByText('Last frame · Reconnecting…')).toBeVisible();
  disconnected = false;
  await expect(page.getByText('Live', { exact: true })).toBeVisible({ timeout: 10000 });
  expect(await image.getAttribute('src')).toBe(previous);
  await page.getByRole('button', { name: 'Back to conversation' }).click();
  await expect(page.getByRole('textbox', { name: 'Message' })).toHaveValue(
    'Preserve the browser conversation',
  );
  await expect.poll(() => releases).toBeGreaterThan(0);
  const closedReads = reads;
  await page.waitForTimeout(1500);
  expect(reads).toBe(closedReads);
  await openComputer.click();
  await expect(page.getByRole('tab', { name: 'Browser', exact: true })).toHaveAttribute(
    'aria-selected',
    'true',
  );
  await expect(page.getByText('Live', { exact: true })).toBeVisible();
  expect(await image.getAttribute('src')).toBe(previous);
  for (const width of [390, 320]) {
    await page.setViewportSize({ width, height: 1000 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
      true,
    );
    await expect(image).toBeVisible();
    await expect
      .poll(() =>
        image.evaluate((element) => {
          const bounds = element.getBoundingClientRect();
          return (
            bounds.left >= 0 &&
            bounds.right <= innerWidth &&
            bounds.top >= 0 &&
            bounds.bottom <= innerHeight
          );
        }),
      )
      .toBe(true);
    await page.screenshot({ path: info.outputPath(`native-mobile-${width}.png`), fullPage: true });
  }
  await page.getByRole('button', { name: 'Back to conversation' }).click();
  const finalRead = await request.get(origins.owner + root + '/browser' + query);
  expect(finalRead.status()).toBe(200);
  const finalFrame = await finalRead.json();
  expect(finalFrame.context_id).toBe(pageB.context_id);
  expect(
    createHash('sha256').update(Buffer.from(finalFrame.frame.data, 'base64')).digest('hex'),
  ).toBe(pageB.sha256);
  await request.delete(origins.owner + root + '/browser' + query);
  expect(boundary.errors).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.writes).toEqual([]);
  const result = {
    passed: true,
    frames,
    reads,
    releases,
    foreignRead: 404,
    viewerEnable: 403,
    staleIncarnation: 409,
    reconnectPreservesPage: true,
    closedPollingStops: true,
    boundary:
      'Real native runtime HTTP, Core authorization, public handlers, SDK and Web; controlled model/auth/store/conversation fixtures; private file RPC substitutes only runtime transport',
  };
  await writeFile(join(directory, 'web-chain-result.json'), JSON.stringify(result, null, 2));
  await writeFile(join(directory, 'stop.json'), '{}');
  await writeFile(join(directory, 'public-stop.json'), '{}');
});
