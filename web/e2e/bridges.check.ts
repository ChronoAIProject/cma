import { test, expect } from '@playwright/test';
import type { BridgeConfigurationDetail } from '../../sdk/typescript/src/index';
import { LocalStartup, recorded } from './agent-startup-fixtures';

test('LOCAL bridge setup preserves edits, shows exact connection failures and limits management to current authority', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  let manager = true;
  let imports = 0;
  let managementReads = 0;
  const detail = recorded<BridgeConfigurationDetail>(
    'bridge-management.json',
    'bridge-setup-configuration-success',
  );
  const id = detail.bridge.id;
  const choices = {
    bots: [{ id: 'bot', platform: 'telegram', active: true }],
    keys: [{ id: 'key', active: true, callback_configured: true }],
    routes: ['route-a', 'route-b'].map((route) => ({
      id: route,
      bot_id: 'bot',
      key_id: 'key',
      platform: 'telegram',
      active: true,
    })),
  };
  await page.route('**/api/v1/sandbox-profiles**', (route) =>
    route.fulfill({ json: { items: [] } }),
  );
  await page.route('**/api/v1/bridge-connectors**', (route) => route.fulfill({ json: choices }));
  await page.route('**/api/v1/bridges**', (route) => {
    const url = new URL(route.request().url());
    if (url.pathname.endsWith('/invoke-readiness'))
      return route.fulfill({
        json: {
          bridge_id: id,
          revision: 1,
          config_revision: 1,
          status: 'not_applicable',
          candidate: null,
        },
      });
    if (url.pathname.endsWith('/invoke-grants') || url.pathname.endsWith('/deliveries'))
      return route.fulfill({ json: { items: [], next_cursor: null } });
    if (url.pathname === '/api/v1/bridges')
      return route.fulfill({ json: { items: [{ ...detail.bridge, can_manage: manager }] } });
    if (url.pathname === `/api/v1/bridges/${id}`)
      return route.fulfill({ json: { ...detail.bridge, can_manage: manager } });
    if (url.pathname.endsWith('/configuration')) {
      managementReads += 1;
      return route.fulfill({ json: detail });
    }
    if (url.pathname.endsWith('/credential')) {
      imports += 1;
      return route.fulfill({
        status: 409,
        json: { code: 'bridge_binding_mismatch', detail: 'NEVER_RENDER_LOCAL_KEY' },
      });
    }
    if (url.pathname.endsWith('/validate'))
      return route.fulfill({
        status: 409,
        json: {
          code: 'bridge_renewable_authority_required',
          detail: 'Scoped credentials unavailable',
        },
      });
    boundary.unexpected.push(`${route.request().method()} ${url.pathname}`);
    return route.fulfill({ status: 500, json: { code: 'unexpected_fixture_request' } });
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/bridges');
  await page.getByRole('link', { name: detail.bridge.display_name, exact: true }).click();
  await expect(page.getByRole('heading', { name: detail.bridge.display_name })).toBeVisible();
  await expect(page.getByText(detail.callback_url, { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Validate connection', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText(
    'permission to use your NyxID connection for 30 days',
  );
  await expect(page.getByRole('button', { name: 'Enable bridge', exact: true })).toBeDisabled();
  const keyInput = page.getByLabel('Import selected Agent key', { exact: true });
  await keyInput.fill('NEVER_RENDER_LOCAL_KEY');
  await page.getByRole('button', { name: 'Import key', exact: true }).click();
  await expect(keyInput).toHaveValue('');
  await expect(page.locator('body')).not.toContainText('NEVER_RENDER_LOCAL_KEY');
  const name = page.getByLabel('Bridge name', { exact: true });
  await name.fill('Preserve my reviewed settings');
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await expect(name).toHaveValue('Preserve my reviewed settings');
  await expect(name).toBeEnabled();
  await page.getByRole('button', { name: 'Resource placement', exact: true }).click();
  await expect(
    page.getByRole('menuitemradio', { name: /A new Agent in an existing Workspace/ }),
  ).toHaveAttribute('aria-disabled', 'true');
  await page.keyboard.press('Escape');
  for (const theme of ['light', 'dark']) {
    await page.evaluate((value) => {
      document.body.toggleAttribute('data-ds-dark-theme', value === 'dark');
      document.documentElement.style.colorScheme = value;
    }, theme);
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      await name.scrollIntoViewIfNeeded();
      await expect
        .poll(() =>
          page.locator('.bridges').evaluate((element) =>
            [...element.querySelectorAll('input,button,code')].every((item) => {
              const rect = item.getBoundingClientRect();
              return rect.width === 0 || (rect.left >= 0 && rect.right <= innerWidth + 1);
            }),
          ),
        )
        .toBe(true);
      await page.screenshot({
        path: info.outputPath(`bridge-setup-${theme}-${width}.png`),
        fullPage: true,
        animations: 'disabled',
      });
    }
  }
  manager = false;
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.getByRole('button', { name: 'Refresh bridge', exact: true }).click();
  await expect(page.getByText(/owner or an organization admin can manage/)).toBeVisible();
  await expect(name).toHaveCount(0);
  const reads = managementReads;
  await page.getByRole('button', { name: 'Refresh bridge', exact: true }).click();
  await expect(page.getByText(/owner or an organization admin can manage/)).toBeVisible();
  expect(managementReads).toBe(reads);
  expect(imports).toBe(1);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
