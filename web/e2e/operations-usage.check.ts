import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';

for (const theme of ['light', 'dark', 'system']) {
  test(`LOCAL measured Operations dashboard in ${theme}`, async ({ page, baseURL }, info) => {
    const boundary = new LocalStartup();
    const metrics = new LocalUsageMetrics();
    await boundary.route(page, baseURL!);
    await page.route('**/auth/session', (route) =>
      route.fulfill({
        json: {
          authenticated: true,
          subject: 'local-owner',
          display_name: 'Local operator',
          capabilities: ['use_own_sandboxes', 'read_all_sandboxes'],
        },
      }),
    );
    await metrics.route(page);
    await page.addInitScript(
      (appearance) => localStorage.setItem('cma:appearance', appearance),
      theme,
    );
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto('/operations?tab=usage&range=7d');
    await expect(page.getByRole('tab', { name: 'Sandboxes & usage' })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    await expect(page.getByRole('tab', { name: 'Usage', exact: true })).toHaveCount(0);
    await expect(page.getByRole('button', { name: 'Research Sandbox', exact: true })).toBeVisible();
    await expect(page.getByText(/25,001 Sandboxes · 18,000 running/)).toBeVisible();
    await expect(page.locator('.recharts-line-curve').first()).toBeVisible();
    await page.locator('.operator-scroll').evaluate((element) => element.scrollTo(0, 0));
    await page.screenshot({
      path: info.outputPath(`operations-${theme}-overview.png`),
      fullPage: true,
      animations: 'disabled',
    });
    expect(metrics.creates).toHaveLength(1);
    expect(metrics.creates[0]!.input.step_seconds).toBe(300);
    expect(metrics.reads.filter((url) => url.pathname.endsWith('/sandboxes'))).toHaveLength(1);
    expect(
      metrics.reads.every(
        (url) => !url.searchParams.has('limit') || url.searchParams.get('limit') === '50',
      ),
    ).toBe(true);
    await page.getByText('View CPU chart data', { exact: true }).focus();
    await page.keyboard.press('Enter');
    await expect(page.getByRole('table', { name: 'CPU chart data' })).toBeVisible();
    await expect(page.getByRole('cell', { name: /no measurement/ })).toBeVisible();
    await page
      .getByRole('button', { name: 'Sort by CPU mean / allocated (cores) ascending' })
      .click();
    await expect(page.getByRole('columnheader', { name: /CPU mean/ })).toHaveAttribute(
      'aria-sort',
      'ascending',
    );
    expect(metrics.creates).toHaveLength(1);
    await page.getByRole('button', { name: 'Next usage page' }).click();
    await expect(
      page.getByRole('button', { name: 'Second page Sandbox', exact: true }),
    ).toBeVisible();
    await page.screenshot({
      path: info.outputPath(`operations-${theme}-desktop.png`),
      fullPage: true,
      animations: 'disabled',
    });
    await page.getByRole('button', { name: 'Owner: Everyone' }).click();
    await page.getByRole('button', { name: 'Next choices' }).click();
    await page.getByRole('button', { name: 'Choose owner' }).click();
    await page.getByRole('menuitemradio', { name: 'Remote owner (owner-25001)' }).click();
    await page.getByRole('button', { name: 'Apply usage filters' }).click();
    await expect.poll(() => metrics.creates.at(-1)?.input.filters?.owner).toBe('owner-25001');
    await expect(page.getByRole('button', { name: 'Research Sandbox', exact: true })).toBeVisible();
    expect(new URL(page.url()).searchParams.get('owner')).toBe('owner-25001');
    await page.getByRole('button', { name: 'Research Sandbox', exact: true }).click();
    await expect(page.getByRole('region', { name: 'Sandbox usage details' })).toBeVisible();
    await page.getByRole('button', { name: 'Lifecycle history', exact: true }).click();
    await expect(page.getByRole('table', { name: 'Usage intervals' })).toBeVisible();
    for (const width of [768, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
      ).toBe(true);
    }
    await page.getByRole('button', { name: 'Close usage details' }).click();
    await page.locator('.operator-scroll').evaluate((element) => element.scrollTo(0, 0));
    await page.screenshot({
      path: info.outputPath(`operations-${theme}-mobile.png`),
      fullPage: true,
      animations: 'disabled',
    });
    await page.getByRole('searchbox', { name: 'Usage Sandbox search' }).fill('empty');
    await page.getByRole('button', { name: 'Apply usage filters' }).click();
    await expect(page.getByText('No Sandboxes match this selection.')).toBeVisible();
    metrics.failure = 503;
    await page.getByRole('button', { name: 'Refresh usage', exact: true }).click();
    await expect(
      page.getByText('Usage reporting is unavailable', { exact: true }).first(),
    ).toBeVisible();
    expect(boundary.writes).toEqual([]);
  });
}
