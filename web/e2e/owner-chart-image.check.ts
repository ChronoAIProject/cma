import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { LocalUsageFilterOptions } from './usage-filter-options.fixture';

for (const theme of ['light', 'dark']) {
  test(`IMAGE owner allocation ranking, bounded scrolling and failure paths ${theme}`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = new LocalStartup();
    const metrics = new LocalUsageMetrics();
    const options = new LocalUsageFilterOptions();
    metrics.ownerCount = 20001;
    const errors: string[] = [];
    page.on('pageerror', (error) => errors.push(error.message));
    await boundary.route(page, baseURL!);
    await metrics.route(page);
    await options.route(page);
    await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
    await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
    await page.goto('/operations?tab=sandboxes');
    await page.getByRole('button', { name: 'Allocation chart grouping', exact: true }).click();
    await page.getByRole('menuitemradio', { name: 'By owner', exact: true }).click();
    const chart = page.getByRole('region', { name: 'Allocation comparison chart' });
    const scroll = page.getByRole('region', { name: 'Scrollable owner allocations' });
    await expect(chart.getByText('10 of 20,001 owners · largest allocation first')).toBeVisible();
    const creates = metrics.creates.length;
    const select = async (name: string) => {
      await chart.getByRole('button', { name: 'Owners shown' }).click();
      await page.getByRole('menuitemradio', { name, exact: true }).click();
    };
    await chart.scrollIntoViewIfNeeded();
    await expect(scroll.locator('.recharts-bar-rectangle').first()).toBeVisible();
    const firstBar = await scroll.locator('.recharts-bar-rectangle').first().boundingBox();
    const firstLabel = await scroll.locator('li').first().boundingBox();
    expect(
      Math.abs(firstBar!.y + firstBar!.height / 2 - firstLabel!.y - firstLabel!.height / 2),
    ).toBeLessThan(3);
    await expect(
      chart.locator('.usage-owner-allocation__axis .recharts-cartesian-axis-tick').first(),
    ).toBeVisible();
    await page.screenshot({ path: info.outputPath(`top-10-duplicates-${theme}.png`) });
    await expect(scroll.getByText('Joined 2026-01-01', { exact: true })).toBeVisible();
    await expect(scroll.getByText('Joined 2026-02-01', { exact: true })).toBeVisible();
    for (const top of ['5', '50', '10']) {
      await select(`Top ${top}`);
      await expect(
        chart.getByText(`${top} of 20,001 owners · largest allocation first`),
      ).toBeVisible();
      await expect(scroll.locator('li')).toHaveCount(Math.min(Number(top), 12));
      await page.screenshot({ path: info.outputPath(`top-${top}-${theme}.png`) });
    }
    await select('All');
    await expect(chart.getByText(/200 of 20,001 owners loaded/)).toBeVisible();
    await page.screenshot({ path: info.outputPath(`all-${theme}.png`) });
    const before = await page.evaluate(() => window.scrollY);
    await scroll.focus();
    await page.keyboard.press('PageDown');
    await expect.poll(() => scroll.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
    expect(await page.evaluate(() => window.scrollY)).toBe(before);
    await page.screenshot({ path: info.outputPath(`scrolled-${theme}.png`) });
    expect(await scroll.locator('li').count()).toBeLessThanOrEqual(12);
    await scroll.evaluate((element) => element.scrollTo(0, element.scrollHeight));
    await expect(chart.getByText(/400 of 20,001 owners loaded/)).toBeVisible();
    expect(metrics.creates).toHaveLength(creates);
    expect(await chart.innerText()).not.toMatch(/91892afa|[0-9a-f]{8}-[0-9a-f]{4}-/);
    expect(
      await chart
        .locator('[title],[aria-label]')
        .evaluateAll((elements) =>
          elements
            .map(
              (element) => `${element.getAttribute('title')} ${element.getAttribute('aria-label')}`,
            )
            .join(' '),
        ),
    ).not.toContain('91892afa');
    metrics.ownerFailure = 503;
    await chart.getByRole('button', { name: 'Load more owners' }).click();
    await expect(chart.getByRole('alert')).toBeVisible();
    await page.screenshot({ path: info.outputPath(`continuation-error-${theme}.png`) });
    metrics.ownerFailure = 0;
    await chart.getByRole('button', { name: 'Retry usage' }).click();
    await expect(chart.getByText(/600 of 20,001 owners loaded/)).toBeVisible();
    await select('Top 5');
    await scroll.getByTitle('Alexandra Montgomery Research Operations', { exact: true }).focus();
    await expect(
      scroll.getByRole('tooltip').filter({ hasText: 'Alexandra Montgomery Research Operations' }),
    ).toBeVisible();
    await page.screenshot({ path: info.outputPath(`long-name-focus-${theme}.png`) });
    expect(metrics.creates).toHaveLength(creates);
    expect(boundary.writes).toEqual([]);
    expect(errors).toEqual([]);
  });
}
