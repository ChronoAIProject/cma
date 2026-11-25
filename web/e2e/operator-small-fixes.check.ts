import { test, expect } from '@playwright/test';
import type { StoredPlatformSettings, WarmPoolList } from '@cma/client';
import { LocalStartup, recorded } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { metricRow } from '../src/usage/metrics/fixtures.test-support';

for (const [theme, width] of [
  ['light', 1440],
  ['dark', 390],
  ['light', 320],
] as const) {
  test(`LOCAL Operations validation and confirmation ${theme} ${width}`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = new LocalStartup();
    const metrics = new LocalUsageMetrics();
    metrics.presentationRows = [metricRow];
    await boundary.route(page, baseURL!);
    await metrics.route(page);
    await metrics.session(page, [
      'use_own_sandboxes',
      'read_all_sandboxes',
      'control_any_sandbox',
      'configure_platform',
    ]);
    let settings = recorded<StoredPlatformSettings>('admin.json', 'admin-settings-read');
    let pools = recorded<WarmPoolList>('native-warm-pools.json', 'native-pools-list-operator');
    const writes: string[] = [];
    await page.route('**/api/v1/admin/settings', async (route) => {
      if (route.request().method() === 'PUT') {
        writes.push('settings');
        settings = {
          ...settings,
          revision: settings.revision + 1,
          settings: route.request().postDataJSON().settings,
        };
      }
      await route.fulfill({ json: settings });
    });
    await page.route('**/api/v1/admin/warm-pools', (route) => route.fulfill({ json: pools }));
    await page.route('**/api/v1/admin/warm-pools/*', async (route) => {
      expect(route.request().method()).toBe('PUT');
      const body = route.request().postDataJSON();
      const pool = pools.items[0]!;
      expect(body).toEqual({ expected_revision: pool.revision, target_replicas: 0 });
      writes.push('disable');
      const updated = { ...pool, target_replicas: 0, revision: pool.revision + 1 };
      pools = { ...pools, items: [updated] };
      await route.fulfill({ status: 202, json: updated });
    });
    await page.setViewportSize({ width, height: 1000 });
    await page.emulateMedia({ colorScheme: theme });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.addInitScript(
      (appearance) => localStorage.setItem('cma:appearance', appearance),
      theme,
    );
    await page.goto('/operations?tab=settings');
    if (theme === 'dark') await expect(page.locator('body')).toHaveAttribute('data-ds-dark-theme');
    else await expect(page.locator('body')).not.toHaveAttribute('data-ds-dark-theme');
    await page.getByRole('tab', { name: 'Platform settings' }).click();
    for (const label of [
      'Agent log retention (days)',
      'Sandboxes per user',
      'Visitor Sandboxes',
      'Visitor launches per day',
    ]) {
      const input = page.getByRole('spinbutton', { name: label, exact: true });
      const original = await input.inputValue();
      await input.fill('-1');
      await expect(input).toHaveAttribute('aria-invalid', 'true');
      const error = await input.evaluate((element: HTMLInputElement) => element.validationMessage);
      await expect(input).toHaveAccessibleDescription(error);
      await page.getByRole('button', { name: 'Save settings', exact: true }).click();
      expect(writes).toEqual([]);
      await input.fill(original);
      await expect(input).toHaveAttribute('aria-invalid', 'false');
    }
    await page.getByRole('button', { name: 'Refresh settings availability' }).click();
    await expect(page.getByRole('button', { name: 'Save settings', exact: true })).toBeEnabled();
    await expect(page.getByText(/Your draft is retained/)).toHaveCount(0);
    await page.getByRole('tab', { name: 'Warm pools' }).click();
    await page.getByText('Configure a warm pool', { exact: true }).click();
    const create = page.locator('.warm-pool-create');
    for (const label of ['CPU (millicores)', 'Memory (MiB)', 'Ready Sandbox target']) {
      const input = create.getByRole('spinbutton', { name: label, exact: true });
      const original = await input.inputValue();
      await input.fill('-1');
      await expect(input).toHaveAttribute('aria-invalid', 'true');
      await expect(create.getByRole('button', { name: 'Save pool target' })).toBeDisabled();
      await expect(input).toHaveAccessibleDescription(
        await input.evaluate((element: HTMLInputElement) => element.validationMessage),
      );
      if (label === 'CPU (millicores)')
        await page.screenshot({ path: info.outputPath('numeric-validation.png'), fullPage: true });
      await input.fill(original);
    }
    const poolRow = page
      .getByRole('table', { name: 'Current runtime pools' })
      .getByRole('row', { name: 'Warm pool 1000 millicores / 1024 MiB', exact: true });
    await poolRow.getByRole('button', { name: 'Disable pool', exact: true }).click();
    await expect(page.getByText(/Disable pool wp_contract/)).toContainText('from 2 to 0');
    const disableDialog = page.getByRole('dialog', { name: 'Disable warm pool' });
    await expect(disableDialog).toBeVisible();
    const bounds = await disableDialog.boundingBox();
    expect(bounds!.x).toBeGreaterThanOrEqual(0);
    expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
    await page.screenshot({ path: info.outputPath('disable-confirmation.png'), fullPage: true });
    expect(writes).toEqual([]);
    await page.getByRole('button', { name: 'Cancel' }).click();
    expect(writes).toEqual([]);
    await poolRow.getByRole('button', { name: 'Disable pool', exact: true }).click();
    await page.getByRole('button', { name: 'Confirm disable' }).click();
    await expect(page.getByText('Target saved.', { exact: true })).toBeVisible();
    expect(writes).toEqual(['disable']);
    await page.getByRole('tab', { name: 'Sandboxes & usage' }).click();
    await page.getByRole('button', { name: 'Retire', exact: true }).click();
    const dialog = page.getByRole('dialog');
    await expect(dialog).toContainText('Owner: Alex Chen');
    await page.screenshot({ path: info.outputPath('retirement-owner.png'), fullPage: true });
    await expect(dialog.getByRole('button', { name: 'Retire Sandbox' })).toBeDisabled();
    await dialog.getByRole('button', { name: 'Cancel' }).click();
    expect(boundary.writes).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}
