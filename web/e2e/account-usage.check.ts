import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';

const measured = process.env.CMA_E2E_MEASURED_USAGE === 'true';

test('LOCAL account Usage preserves the Agent, owner scope, focus and browser history', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  const metrics = new LocalUsageMetrics();
  await boundary.route(page, baseURL!);
  await metrics.route(page);
  metrics.measuredVisible = measured;
  await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/agents/agt_contract');
  const editor = page.getByRole('textbox', { name: 'Message', exact: true });
  await editor.fill('Keep this unsent draft');
  await editor.evaluate((node) => node.setAttribute('data-retained-usage-proof', 'true'));
  const trigger = page.getByRole('button', { name: /Account menu for/ });
  await trigger.click();
  await expect(page.getByRole('menuitem', { name: 'Settings', exact: true })).toHaveCount(0);
  const appearance = page.getByRole('group', { name: 'Appearance' });
  for (const theme of ['System', 'Light', 'Dark']) {
    await appearance.getByRole('menuitemradio', { name: theme, exact: true }).click();
    await expect(
      appearance.getByRole('menuitemradio', { name: theme, exact: true }),
    ).toHaveAttribute('aria-checked', 'true');
  }
  await page.getByRole('menuitem', { name: 'Usage', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Your usage' });
  await expect(dialog.getByRole('button', { name: 'Research Sandbox', exact: true })).toBeVisible();
  expect(metrics.creates.every((read) => !read.admin && !read.input.filters?.owner)).toBe(true);
  await expect(page.getByRole('dialog')).toHaveCount(1);
  if (measured) {
    await dialog.getByRole('button', { name: 'Memory working set', exact: true }).click();
    await expect(
      dialog.getByText('View Memory working set chart data', { exact: true }),
    ).toBeVisible();
  } else {
    await expect(dialog.getByRole('region', { name: 'Allocated capacity now' })).toContainText(
      '2 cores',
    );
    await expect(dialog.getByRole('region', { name: 'Allocated resource-hours' })).toContainText(
      '480',
    );
    await expect(dialog.getByRole('button', { name: 'Memory working set' })).toHaveCount(0);
    await expect(dialog.getByRole('region', { name: 'Measured resource charts' })).toHaveCount(0);
  }
  expect(metrics.creates).toHaveLength(1);
  await dialog.getByRole('button', { name: 'Research Sandbox', exact: true }).click();
  await dialog.getByRole('button', { name: 'Lifecycle history', exact: true }).click();
  await expect(dialog.getByRole('table', { name: 'Usage intervals' })).toBeVisible();
  await dialog.getByRole('button', { name: 'Close usage details' }).click();
  for (const theme of ['light', 'dark', 'system']) {
    await page.emulateMedia({ colorScheme: theme === 'dark' ? 'dark' : 'light' });
    await page.evaluate((value) => {
      localStorage.setItem('cma:appearance', value);
      window.dispatchEvent(new StorageEvent('storage', { key: 'cma:appearance' }));
    }, theme);
    for (const width of [1440, 768, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      await dialog.locator('.modal__body').evaluate((node) => node.scrollTo(0, 0));
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
      ).toBe(true);
      await expect(dialog.getByRole('button', { name: 'Close', exact: true })).toBeInViewport();
      await page.screenshot({
        path: info.outputPath(`usage-${theme}-${width}.png`),
        animations: 'disabled',
      });
    }
  }
  await page.setViewportSize({ width: 1440, height: 1000 });
  await dialog.getByRole('button', { name: 'Close', exact: true }).focus();
  await page.keyboard.press('Shift+Tab');
  expect(await dialog.evaluate((node) => node.contains(document.activeElement))).toBe(true);
  await page.keyboard.press('Escape');
  await expect(dialog).not.toBeVisible();
  await expect(page).toHaveURL(/\/agents\/agt_contract$/);
  await expect(editor).toHaveValue('Keep this unsent draft');
  await expect(editor).toHaveAttribute('data-retained-usage-proof', 'true');
  await expect(trigger).toBeFocused();
  await page.goForward();
  await expect(dialog).toBeVisible();
  await page.goBack();
  await expect(dialog).not.toBeVisible();
  if (!measured)
    expect(metrics.reads.filter((url) => url.pathname.endsWith('/series'))).toEqual([]);
  expect(boundary.writes).toEqual([]);
  expect(boundary.errors).toEqual([]);
});

for (const path of ['/usage', '/settings/usage', '/settings/general', '/settings']) {
  test(`LOCAL direct usage and legacy link ${path}`, async ({ page, baseURL }) => {
    const boundary = new LocalStartup();
    const metrics = new LocalUsageMetrics();
    await boundary.route(page, baseURL!);
    await metrics.route(page);
    metrics.measuredVisible = measured;
    await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
    await page.addInitScript(() => localStorage.setItem('cma:appearance', 'light'));
    await page.goto(`${path}?range=7d`);
    const dialog = page.getByRole('dialog', { name: 'Your usage' });
    await expect(dialog).toBeVisible();
    await expect(page).toHaveURL(/\/usage\?range=7d$/);
    await expect(dialog.getByText('Appearance is now in your account menu.')).toHaveCount(
      path === '/settings' || path === '/settings/general' ? 1 : 0,
    );
    expect(await page.evaluate(() => localStorage.getItem('cma:appearance'))).toBe('light');
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await expect(page).toHaveURL(/\/agent-manager$/);
    if (!measured)
      expect(metrics.reads.filter((url) => url.pathname.endsWith('/series'))).toEqual([]);
    expect(boundary.writes).toEqual([]);
  });
}

for (const status of [403, 410, 429, 503]) {
  test(`LOCAL owner usage reports ${status} without claiming zero usage`, async ({
    page,
    baseURL,
  }) => {
    const boundary = new LocalStartup();
    const metrics = new LocalUsageMetrics();
    metrics.failure = status;
    await boundary.route(page, baseURL!);
    await metrics.route(page);
    metrics.measuredVisible = measured;
    await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
    await page.goto('/usage');
    const dialog = page.getByRole('dialog', { name: 'Your usage' });
    await expect(dialog.getByRole('alert')).toBeVisible();
    await expect(dialog.getByText('No Sandboxes match this selection.')).toHaveCount(0);
    if (status === 429)
      await expect(dialog.getByRole('button', { name: 'Waiting to retry' })).toBeDisabled();
    if (!measured)
      expect(metrics.reads.filter((url) => url.pathname.endsWith('/series'))).toEqual([]);
    expect(boundary.writes).toEqual([]);
  });
}

test('LOCAL pending owner report resolves and empty selections are explicit', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  const metrics = new LocalUsageMetrics();
  metrics.pending = true;
  await boundary.route(page, baseURL!);
  await metrics.route(page);
  metrics.measuredVisible = measured;
  await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
  await page.goto('/usage');
  const dialog = page.getByRole('dialog', { name: 'Your usage' });
  await expect(dialog.getByText('Preparing usage report…')).toBeVisible();
  await expect(dialog.getByRole('button', { name: 'Research Sandbox', exact: true })).toBeVisible();
  expect(metrics.creates).toHaveLength(1);
  metrics.pending = false;
  await dialog.getByRole('searchbox', { name: 'Usage Sandbox search' }).fill('empty');
  await dialog.getByRole('button', { name: 'Apply usage filters' }).click();
  await expect(dialog.getByText('No Sandboxes match this selection.')).toBeVisible();
  if (!measured)
    expect(metrics.reads.filter((url) => url.pathname.endsWith('/series'))).toEqual([]);
  expect(boundary.writes).toEqual([]);
});

test('LOCAL mobile Usage returns focus to the retained account trigger', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  const metrics = new LocalUsageMetrics();
  await boundary.route(page, baseURL!);
  await metrics.route(page);
  metrics.measuredVisible = measured;
  await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto('/agents/agt_contract');
  await page.getByRole('button', { name: 'Show navigation', exact: true }).click();
  const trigger = page.getByRole('button', { name: /Account menu for/ });
  await trigger.click();
  await page.getByRole('menuitem', { name: 'Usage', exact: true }).click();
  const dialog = page.getByRole('dialog', { name: 'Your usage' });
  await expect(dialog).toBeVisible();
  await expect(page.getByRole('dialog')).toHaveCount(1);
  await page.keyboard.press('Escape');
  await expect(dialog).not.toBeVisible();
  await expect(trigger).toBeFocused();
  await expect(trigger).toBeInViewport();
  if (!measured)
    expect(metrics.reads.filter((url) => url.pathname.endsWith('/series'))).toEqual([]);
  expect(boundary.writes).toEqual([]);
});
