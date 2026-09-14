import { join } from 'node:path';
import { test, expect } from '@playwright/test';
import { settings } from './environment';
import { assertPageSafe, readOnlyGuard } from './security';

test('SMOKE public discovery and landing are read-only', async ({ page }) => {
  const config = settings();
  const blocked: string[] = [];
  await readOnlyGuard(page.context(), blocked);
  const discovery = await page.request.get(`${config.baseURL}/api/v1/conformance`);
  expect(discovery.ok()).toBe(true);
  expect((await discovery.json()).service).toBe('cma');
  await page.goto(`${config.baseURL}/t/${config.publicId}`);
  await expect(page.locator('.public-heading h1')).toBeVisible();
  await expect(page.locator('.public-actions button')).toBeEnabled();
  await assertPageSafe(page, config.baseURL, []);
  await page.screenshot({ path: join(config.output, 'smoke-landing.png') });
  await page.reload();
  await expect(page.locator('.public-heading h1')).toBeVisible();
  expect(blocked).toEqual([]);
});

test('SECURITY anonymous badge sets no cookie and references no credentials', async ({
  request,
}) => {
  const config = settings();
  const response = await request.get(
    `${config.baseURL}/api/v1/triggers/public/${config.publicId}/badge.svg`,
  );
  expect(response.status()).toBe(200);
  expect(response.headers()['content-type']).toContain('image/svg+xml');
  expect(response.headers()['set-cookie']).toBeUndefined();
  expect(await response.text()).toContain('<svg');
});

test('SMOKE anonymous console stays at the passive sign-in gate', async ({ page }) => {
  const config = settings();
  const blocked: string[] = [];
  await readOnlyGuard(page.context(), blocked);
  await page.goto(`${config.baseURL}/`);
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
  await assertPageSafe(page, config.baseURL, []);
  expect(blocked).toEqual([]);
});
