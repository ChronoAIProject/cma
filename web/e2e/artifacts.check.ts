import { createHash } from 'node:crypto';
import { test, expect } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

test('LOCAL approved artifacts preserve personal work, publish once and revoke current downloads', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  boundary.sandbox = { ...boundary.sandbox, owner: 'local-owner' };
  await boundary.route(page, baseURL!);
  const artifact = {
    id: `art_${'a'.repeat(64)}`,
    agent_id: 'agt_contract',
    name: 'Review.txt',
    media_type: 'text/plain',
    size_bytes: 6,
    sha256: createHash('sha256').update('review').digest('hex'),
    created_at: localTime,
  };
  let revoked = false;
  let published = false;
  let lost = false;
  let accessReads = 0;
  const writes: { body: unknown; key: string | undefined }[] = [];
  await page.route('**/auth/connections/check', (route) =>
    route.fulfill({
      json: { owner: 'local-owner', ready: true, checked_at: localTime, services: [] },
    }),
  );
  await page.route('**/api/v1/resource-access/agent/agt_contract', (route) => {
    accessReads += 1;
    return route.fulfill(
      revoked
        ? { status: 404, json: { code: 'resource_not_found', detail: 'Artifact access removed' } }
        : {
            json: {
              capabilities: ['view_details', 'view_history', 'view_artifacts', 'manage_sharing'],
              revalidate_at: null,
              policy_revision: 1,
            },
          },
    );
  });
  await page.route('**/api/v1/agents/agt_contract/artifacts**', async (route) => {
    const request = route.request();
    if (revoked)
      return route.fulfill({
        status: 404,
        json: { code: 'resource_not_found', detail: 'Artifact access removed' },
      });
    if (request.method() === 'POST') {
      writes.push({ body: request.postDataJSON(), key: request.headers()['idempotency-key'] });
      published = true;
      if (!lost) {
        lost = true;
        return route.abort('failed');
      }
      return route.fulfill({ status: 201, json: artifact });
    }
    if (new URL(request.url()).pathname.endsWith('/download'))
      return route.fulfill({ contentType: 'application/octet-stream', body: 'review' });
    return route.fulfill({ json: { items: published ? [artifact] : [] } });
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/agents/agt_contract');
  const composer = page.getByRole('textbox', { name: 'Message' });
  await composer.fill('Keep this draft while reviewing files');
  const node = await composer.elementHandle();
  await page.getByRole('button', { name: 'More Agent actions' }).click();
  await page.getByRole('menuitem', { name: 'Show Agent details' }).click();
  await page.getByRole('button', { name: 'Artifacts', exact: true }).click();
  const modal = page.getByRole('dialog', { name: 'Agent artifacts' });
  await expect(modal.getByText('No published artifacts on this page.')).toBeVisible();
  await modal
    .getByLabel('Artifact file')
    .setInputFiles({ name: 'Review.txt', mimeType: 'text/plain', buffer: Buffer.from('review') });
  await expect(modal.getByRole('button', { name: 'Publish artifact', exact: true })).toBeDisabled();
  await modal.getByRole('checkbox').check();
  const fileInput = await modal.getByLabel('Artifact file').elementHandle();
  const beforeFocus = accessReads;
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await expect.poll(() => accessReads).toBeGreaterThan(beforeFocus);
  await expect(modal.getByRole('button', { name: 'Publish artifact', exact: true })).toBeEnabled();
  expect(
    await modal
      .getByLabel('Artifact file')
      .evaluate((element, previous) => element === previous, fileInput),
  ).toBe(true);
  await expect(modal.getByRole('checkbox')).toBeChecked();
  await modal.getByRole('button', { name: 'Publish artifact', exact: true }).click();
  await expect(modal.getByRole('button', { name: 'Retry artifact publication' })).toBeEnabled();
  await modal.getByRole('button', { name: 'Close', exact: true }).click();
  await page.getByRole('button', { name: 'Close Agent details', exact: true }).click();
  await page.getByRole('button', { name: 'More Agent actions' }).click();
  await page.getByRole('menuitem', { name: 'Show Agent details' }).click();
  await page.getByRole('button', { name: 'Artifacts', exact: true }).click();
  await modal.getByRole('button', { name: 'Retry artifact publication' }).click();
  await expect(modal.getByText('Artifact published.')).toBeVisible();
  expect(writes).toHaveLength(2);
  expect(writes[1]).toEqual(writes[0]);
  const download = page.waitForEvent('download');
  await modal.getByRole('button', { name: 'Download Review.txt' }).click();
  const file = await download;
  expect(file.suggestedFilename()).toBe('Review.txt');
  const stream = await file.createReadStream();
  const chunks = [];
  for await (const chunk of stream) chunks.push(chunk);
  expect(Buffer.concat(chunks).toString()).toBe('review');
  for (const theme of ['light', 'dark']) {
    await page.evaluate((value) => {
      document.body.toggleAttribute('data-ds-dark-theme', value === 'dark');
      document.documentElement.style.colorScheme = value;
    }, theme);
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      await expect
        .poll(() =>
          modal.evaluate((element) => element.getBoundingClientRect().right <= innerWidth + 1),
        )
        .toBe(true);
      await expect
        .poll(() =>
          modal.evaluate((element) =>
            [...element.querySelectorAll('button,input')].every(
              (item) => item.getBoundingClientRect().right <= innerWidth + 1,
            ),
          ),
        )
        .toBe(true);
      await page.screenshot({
        path: info.outputPath(`artifacts-${theme}-${width}.png`),
        fullPage: true,
        animations: 'disabled',
      });
    }
  }
  revoked = true;
  await modal.getByRole('button', { name: 'Refresh artifacts' }).click();
  await expect(modal.getByRole('button', { name: 'Download Review.txt' })).toHaveCount(0);
  await expect(modal.getByRole('button', { name: 'Publish artifact', exact: true })).toHaveCount(0);
  await modal.getByRole('button', { name: 'Close', exact: true }).click();
  await page.setViewportSize({ width: 1440, height: 1000 });
  expect(await composer.evaluate((element, previous) => element === previous, node)).toBe(true);
  await expect(composer).toHaveValue('Keep this draft while reviewing files');
  expect(boundary.unexpected).toEqual([]);
});
