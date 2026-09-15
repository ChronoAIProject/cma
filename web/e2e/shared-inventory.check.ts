import { test, expect } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

test('LOCAL shared aggregate preserves personal work and supports filtered pages and safe exports', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  boundary.sandbox = { ...boundary.sandbox, owner: 'local-owner' };
  await boundary.route(page, baseURL!);
  await page.route('**/auth/connections/check', (route) =>
    route.fulfill({
      json: { owner: 'local-owner', ready: true, checked_at: localTime, services: [] },
    }),
  );
  let revoked = false;
  const reads: URL[] = [];
  const rows = [
    {
      agent_id: 'agt_shared_a',
      title: 'Shared investigation',
      status: 'ready',
      created_at: localTime,
      updated_at: localTime,
      creation_origin: 'manual',
    },
    {
      agent_id: 'agt_shared_b',
      title: 'Shared delivery',
      status: 'stopped',
      created_at: localTime,
      updated_at: localTime,
      creation_origin: 'trigger',
    },
  ] as const;
  await page.route('**/api/v1/organizations', (route) =>
    route.fulfill({
      json: {
        items: revoked ? [] : [{ id: 'org_research', display_name: 'Research', role: 'viewer' }],
      },
    }),
  );
  await page.route('**/api/v1/agents/provenance?*', (route) => {
    const url = new URL(route.request().url());
    reads.push(url);
    expect(route.request().method()).toBe('GET');
    expect(url.searchParams.get('shared')).toBe('true');
    expect(url.searchParams.has('organization')).toBe(false);
    if (revoked)
      return route.fulfill({
        status: 404,
        json: { code: 'resource_not_found', detail: 'Shared access removed' },
      });
    if (url.searchParams.has('creation_origin')) {
      return route.fulfill({
        json: url.searchParams.has('cursor')
          ? { items: [rows[1]] }
          : { items: [], next: rows[0].agent_id },
      });
    }
    return route.fulfill({ json: { items: rows } });
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/agents/agt_contract');
  const composer = page.getByRole('textbox', { name: 'Message' });
  await composer.fill('Keep this personal draft');
  const composerNode = await composer.elementHandle();
  await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Navigation scope', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Shared with me', exact: true }).click();
  const navigation = page.locator('.scoped-navigation');
  await expect(navigation.getByRole('link', { name: rows[0].title })).toBeVisible();
  revoked = true;
  await navigation.getByRole('button', { name: 'Refresh shared Agents' }).click();
  await expect(navigation.getByRole('alert')).toContainText('Shared access removed');
  await expect(navigation.getByText(rows[0].title)).toHaveCount(0);
  expect(await composer.evaluate((element, previous) => element === previous, composerNode)).toBe(
    true,
  );
  await expect(composer).toHaveValue('Keep this personal draft');
  await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
  revoked = false;
  await navigation.getByRole('link', { name: 'All shared Agents' }).click();
  await expect(page).toHaveURL(/\/shared\/agents$/);
  const inventory = page.locator('.agent-inventory');
  await expect(inventory.getByRole('link', { name: rows[0].title })).toBeVisible();
  await page.reload();
  await expect(inventory.getByRole('button', { name: 'Resource scope' })).toContainText(
    'Shared with me',
  );
  await inventory.getByRole('button', { name: 'Creation origin' }).click();
  await page.getByRole('menuitemradio', { name: 'Created by Trigger', exact: true }).click();
  await expect(
    inventory.getByText('No matches on this page. Continue to check the next page.'),
  ).toBeVisible();
  await inventory.getByRole('button', { name: 'Next page' }).click();
  await expect(inventory.getByRole('link', { name: rows[1].title })).toHaveAttribute(
    'href',
    '/agents/agt_shared_b',
  );
  const download = page.waitForEvent('download');
  const beforeExport = reads.length;
  await inventory.getByRole('button', { name: 'Export this page' }).click();
  const file = await download;
  expect(reads.length).toBeGreaterThan(beforeExport);
  const stream = await file.createReadStream();
  const chunks = [];
  for await (const chunk of stream) chunks.push(chunk);
  const exported = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  expect(exported).toEqual({ agents: [rows[1]] });
  for (const theme of ['light', 'dark']) {
    await page.evaluate((value) => {
      document.body.toggleAttribute('data-ds-dark-theme', value === 'dark');
      document.documentElement.style.colorScheme = value;
    }, theme);
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
        .toBe(true);
      await expect
        .poll(() =>
          page
            .locator('.frame__center')
            .evaluate((element) => element.getBoundingClientRect().right <= innerWidth + 1),
        )
        .toBe(true);
      await expect
        .poll(() =>
          inventory.evaluate((element) =>
            [...element.querySelectorAll('button, a, time')].every(
              (item) => item.getBoundingClientRect().right <= innerWidth + 1,
            ),
          ),
        )
        .toBe(true);
      await page.screenshot({
        path: info.outputPath(`shared-inventory-${theme}-${width}.png`),
        fullPage: true,
        animations: 'disabled',
      });
    }
  }
  revoked = true;
  await inventory.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect(inventory.getByRole('alert')).toContainText('Shared access removed');
  await expect(inventory.getByRole('link', { name: rows[1].title })).toHaveCount(0);
  expect(boundary.unexpected).toEqual([]);
});
