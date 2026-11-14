import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { LocalUsageFilterOptions } from './usage-filter-options.fixture';

for (const admin of [false, true]) {
  test(`LOCAL ${admin ? 'Operations' : 'account'} preserves selection through reporting backoff`, async ({
    page,
    baseURL,
  }) => {
    const boundary = new LocalStartup();
    const metrics = new LocalUsageMetrics();
    await boundary.route(page, baseURL!);
    await metrics.route(page);
    await new LocalUsageFilterOptions().route(page);
    await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
    let failure = 0;
    let attempts = 0;
    await page.route('**/api/v1/**usage/metrics/reports', (route) => {
      attempts += 1;
      return failure
        ? route.fulfill({
            status: failure,
            headers: { 'Retry-After': '30' },
            json: { status: failure, title: 'Reporting unavailable', code: 'usage_unavailable' },
          })
        : route.fallback();
    });
    await page.goto(admin ? '/operations?tab=sandboxes' : '/agents/agt_contract');
    const editor = page.getByRole('textbox', { name: 'Message', exact: true });
    if (!admin) {
      await editor.fill('Preserve the unsent draft during reporting backoff');
      await page.getByRole('button', { name: /Account menu for/ }).click();
      await page.getByRole('menuitem', { name: 'Usage', exact: true }).click();
    }
    await expect(page.getByRole('button', { name: 'Research Sandbox', exact: true })).toBeVisible();
    // Freeze before the failure so navigation/click time cannot consume the
    // final millisecond of the server's 30-second Retry-After floor.
    const clockStart = await page.evaluate(() => Date.now());
    await page.clock.install({ time: clockStart - 60_000 });
    await page.clock.pauseAt(clockStart);
    failure = 429;
    await page.getByRole('button', { name: 'Refresh usage', exact: true }).click();
    await expect(page.getByText('Retrying usage automatically…', { exact: true })).toBeVisible();
    expect(attempts).toBe(2);
    await page.getByRole('button', { name: 'Usage time range' }).click();
    await page.getByRole('menuitemradio', { name: 'Last 7 days', exact: true }).click();
    await page.getByRole('button', { name: 'Apply usage filters' }).click();
    await expect(page.getByText('Showing Last 24 hours', { exact: true })).toBeVisible();
    await expect(page.getByText(/not last 7 days/)).toBeVisible();
    await page.getByRole('button', { name: 'Refresh usage', exact: true }).click();
    expect(attempts).toBe(2);
    expect(new URL(page.url()).searchParams.get('range')).toBe('7d');
    if (admin) {
      await page.getByRole('button', { name: 'Owner: Everyone' }).click();
      await expect(
        page.getByRole('menu', { name: 'Owner choices' }).getByText('Loading owner choices…'),
      ).toBeVisible();
      await page.keyboard.press('Escape');
    } else {
      await page.keyboard.press('Escape');
      await expect(editor).toHaveValue('Preserve the unsent draft during reporting backoff');
      await page.goForward();
      await expect(page.getByText('Showing Last 24 hours', { exact: true })).toBeVisible();
    }
    await page.clock.fastForward(29999);
    expect(attempts).toBe(2);
    failure = 0;
    await page.clock.fastForward(4000);
    await expect(page.locator('.usage-metrics')).toHaveAttribute('data-usage-state', 'committed');
    await expect(page.getByRole('button', { name: 'Research Sandbox', exact: true })).toBeVisible();
    expect(attempts).toBe(3);
    expect(metrics.creates.at(-1)?.input.step_seconds).toBe(300);
    expect(metrics.creates.every((request) => request.admin === admin)).toBe(true);
    expect(boundary.writes).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}
