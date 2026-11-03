import { expect, test } from '@playwright/test';
import { createHash } from 'node:crypto';
import { readFile, rename, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import type { BrowserObservation } from '../../sdk/typescript/src/index';
import { LocalStartup, localTime } from './agent-startup-fixtures';

test('NATIVE Home lost receipt recovers one formal task and its prepared browser through public Web', async ({
  page,
  baseURL,
}, info) => {
  const directory = process.env.CMA_DESKTOP_RELAY_DIRECTORY!;
  const publicServer = JSON.parse(await readFile(join(directory, 'public-ready.json'), 'utf8'));
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  const writes: { body: unknown; key: string | undefined; receipt: unknown }[] = [];
  const observations: BrowserObservation[] = [];
  const lifecycleEvents: unknown[] = [];
  let submittedAt = 0;
  await page.route('**/auth/session', (route) =>
    route.fulfill({
      json: {
        authenticated: true,
        subject: 'owner',
        display_name: 'Native task owner',
        capabilities: ['use_own_sandboxes', 'create_sandbox', 'delete_own_sandbox'],
      },
    }),
  );
  await page.route('**/auth/connections/check', (route) =>
    route.fulfill({
      json: {
        owner: 'owner',
        ready: true,
        checked_at: localTime,
        services: [],
      },
    }),
  );
  await page.route(
    /\/(openapi\.json|api\/(agent-manager\/)?v1\/(desktop-capability|agents|agent-lifecycle|sandboxes|workspaces)(\/|\?|$))/,
    async (route) => {
      const outgoing = route.request();
      const url = new URL(outgoing.url());
      const path = url.pathname.replace('/api/agent-manager/v1/', '/api/v1/');
      if (path === '/api/v1/agents' && outgoing.method() === 'POST' && !submittedAt)
        submittedAt = Date.now();
      const response = await route.fetch({
        url: publicServer.origin + path + url.search,
        headers: { ...outgoing.headers(), authorization: 'Bearer local-sdk-conformance' },
      });
      if (path.startsWith('/api/v1/agent-lifecycle/') && response.ok())
        lifecycleEvents.push(await response.json());
      if (path === '/api/v1/agents' && outgoing.method() === 'POST') {
        expect(response.status()).toBe(202);
        const receipt = await response.json();
        const record = {
          body: outgoing.postDataJSON(),
          key: outgoing.headers()['idempotency-key'],
          receipt,
        };
        writes.push(record);
        if (writes.length === 1) {
          expect(record.body).toEqual(publicServer.body);
          await writeFile(join(directory, 'creation.tmp'), JSON.stringify(record));
          await rename(join(directory, 'creation.tmp'), join(directory, 'creation.json'));
          return route.abort('failed');
        }
        expect(record).toEqual(writes[0]);
      }
      if (path.endsWith('/browser') && response.ok() && outgoing.method() === 'GET') {
        expect(response.headers()['cache-control']).toBe('private, no-store');
        observations.push(await response.json());
      }
      await route.fulfill({ response });
    },
  );
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/');
  const toggle = page.getByRole('checkbox', { name: 'Enable desktop' });
  await expect(toggle).toBeEnabled();
  await expect(toggle).not.toBeChecked();
  expect(writes).toHaveLength(0);
  await page.screenshot({ path: info.outputPath('home-default-off.png') });
  await page.getByLabel('Your task').fill(publicServer.body.initial_message);
  await toggle.check();
  expect(writes).toHaveLength(0);
  await page.getByRole('button', { name: 'Start task', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Recover original task' })).toBeVisible();
  await expect(toggle).toBeChecked();
  await expect(toggle).toBeDisabled();
  await page.screenshot({ path: info.outputPath('home-lost-receipt.png') });
  await page.reload();
  await expect(page.getByRole('checkbox', { name: 'Enable desktop' })).toBeChecked();
  await page.getByRole('button', { name: 'Recover original task' }).click();
  await expect(page).toHaveURL(/\/agents\/agt_/, { timeout: 90000 });
  const agentVisibleAt = Date.now();
  const agentId = new URL(page.url()).pathname.split('/').at(-1)!;
  await expect(
    page.getByRole('heading', { name: 'Agent unavailable', exact: true }),
  ).not.toBeVisible();
  const open = page.getByRole('button', { name: 'Show Agent computer', exact: true });
  if (await open.isVisible()) await open.click();
  await page.getByRole('tab', { name: 'Browser', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Enable browser', exact: true })).not.toBeVisible();
  await expect(page.getByText('Live', { exact: true })).toBeVisible({ timeout: 90000 });
  const firstLiveAt = Date.now();
  const image = page.getByAltText('Agent browser page');
  await expect(image).toHaveJSProperty('naturalWidth', 1280);
  await expect(image).toHaveJSProperty('naturalHeight', 720);
  await expect
    .poll(
      async () => {
        const progress = await readFile(join(directory, 'native-progress.json'), 'utf8').catch(
          () => '{}',
        );
        return JSON.parse(progress).completions;
      },
      { timeout: 60000 },
    )
    .toBe(1);
  await expect
    .poll(() => observations.filter((value) => value.frame).at(-1)?.captures)
    .toBeGreaterThan(1);
  const first = observations.filter((value) => value.frame).at(-1)!;
  const imageBytes = Buffer.from(first.frame!.data, 'base64');
  await writeFile(info.outputPath('actual-native-frame.jpg'), imageBytes);
  for (const width of [1440, 390, 320]) {
    await page.setViewportSize({ width, height: 1000 });
    await expect(image).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
      true,
    );
    await expect
      .poll(async () => {
        const bounds = await image.boundingBox();
        return bounds !== null && bounds.x >= 0 && bounds.x + bounds.width <= width;
      })
      .toBe(true);
    await page.screenshot({
      path: info.outputPath(`native-desktop-${width}.png`),
      animations: 'disabled',
    });
  }
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.reload();
  const reopen = page.getByRole('button', { name: 'Show Agent computer', exact: true });
  if (await reopen.isVisible()) await reopen.click();
  await page.getByRole('tab', { name: 'Browser', exact: true }).click();
  await expect(page.getByText('Live', { exact: true })).toBeVisible({ timeout: 15000 });
  await expect.poll(() => observations.at(-1)?.context_id).toBe(first.context_id);
  expect(writes).toHaveLength(2);
  const result = {
    passed: true,
    submittedAt,
    agentVisibleAt,
    firstLiveAt,
    agentVisibleAfterMs: agentVisibleAt - submittedAt,
    firstLiveAfterMs: firstLiveAt - submittedAt,
    agentId,
    creation: writes[0],
    replays: writes.length - 1,
    contextId: first.context_id,
    runtimeIncarnation: first.runtime_incarnation,
    frameSha256: createHash('sha256').update(imageBytes).digest('hex'),
    frameBytes: imageBytes.length,
    observations: observations.length,
    lifecycleEvents,
    startupFixture:
      'Local IdP, optional application startup projections and external provider/home allocation; real Mongo, lifecycle, workers, native runtime, ACP, browser, public handlers and SDK.',
  };
  await writeFile(join(directory, 'web-result.json'), JSON.stringify(result, null, 2));
});
