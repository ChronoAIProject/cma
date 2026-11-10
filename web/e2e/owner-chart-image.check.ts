import { test, expect, type Locator } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { LocalUsageFilterOptions } from './usage-filter-options.fixture';

const longName =
  'Alexandra Montgomery Research Operations — International Infrastructure Allocation and Capacity Planning Team';

async function fullTooltip(tooltip: Locator) {
  await expect(tooltip).toBeVisible();
  await expect(tooltip).toBeInViewport({ ratio: 1 });
  expect(
    await tooltip.evaluate((element) => {
      const box = element.getBoundingClientRect();
      return (
        element.parentElement === document.body &&
        box.top >= 0 &&
        box.bottom <= innerHeight &&
        box.left >= 0 &&
        box.right <= innerWidth &&
        element.scrollHeight <= element.clientHeight
      );
    }),
  ).toBe(true);
}

for (const theme of ['light', 'dark']) {
  test(`IMAGE owner allocation ranking, bounded scrolling and failure paths ${theme}`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = new LocalStartup();
    const metrics = new LocalUsageMetrics();
    const options = new LocalUsageFilterOptions();
    metrics.ownerCount = 20001;
    metrics.ownerOverrides.set(4, { label: longName });
    metrics.ownerOverrides.set(199, {
      label: 'Boundary Alex',
      disambiguator: 'Joined 2026-01-01',
      allocated_hours: 19802,
    });
    metrics.ownerOverrides.set(200, {
      label: 'Boundary Alex',
      disambiguator: 'Joined 2026-02-01',
      allocated_hours: 19802,
    });
    metrics.ownerOverrides.set(207, { label: longName });
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
    const labels = scroll.getByRole('list', { name: 'Owner allocation values' });
    await expect(chart.getByText('10 of 20,001 owners · largest allocation first')).toBeVisible();
    const creates = metrics.creates.length;
    const select = async (name: string) => {
      await chart.getByRole('button', { name: 'Owners shown' }).click();
      await page.getByRole('menuitemradio', { name, exact: true }).click();
    };
    await chart.scrollIntoViewIfNeeded();
    await expect(scroll.locator('.recharts-bar-rectangle').first()).toBeVisible();
    // A value tooltip also contains a list item inside the scroll region.
    await scroll.locator('.recharts-bar-rectangle').first().hover();
    await expect(scroll.locator('.recharts-tooltip-item').first()).toBeVisible();
    const firstBar = await scroll.locator('.recharts-bar-rectangle').first().boundingBox();
    const firstLabel = await labels.getByRole('listitem').first().boundingBox();
    expect(
      Math.abs(firstBar!.y + firstBar!.height / 2 - firstLabel!.y - firstLabel!.height / 2),
    ).toBeLessThan(3);
    const ticks = chart.locator(
      '.usage-owner-allocation__axis .recharts-cartesian-axis-tick-value',
    );
    await expect(ticks).toHaveText(['0', '5K', '10K', '15K', '20K']);
    for (const tick of await ticks.all()) await expect(tick).toBeVisible();
    const grid = scroll.locator('.recharts-cartesian-grid-vertical line');
    await expect(grid).toHaveCount(5);
    for (const index of [1, 2, 3]) {
      // Measure SVG geometry in the same viewport coordinate system in every engine.
      const label = await ticks
        .nth(index)
        .evaluate((element) => element.getBoundingClientRect().toJSON());
      const line = await grid
        .nth(index)
        .evaluate((element) => element.getBoundingClientRect().toJSON());
      expect(Math.abs(label.x + label.width / 2 - line.x)).toBeLessThan(2);
    }
    await page.screenshot({ path: info.outputPath(`top-10-duplicates-${theme}.png`) });
    await expect(scroll.getByText('Joined 2026-01-01', { exact: true })).toBeVisible();
    await expect(scroll.getByText('Joined 2026-02-01', { exact: true })).toBeVisible();
    for (const top of ['5', '50', '10']) {
      await select(`Top ${top}`);
      await expect(
        chart.getByText(`${top} of 20,001 owners · largest allocation first`),
      ).toBeVisible();
      await expect(labels.getByRole('listitem')).toHaveCount(Math.min(Number(top), 12));
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
    expect(await labels.getByRole('listitem').count()).toBeLessThanOrEqual(12);
    await scroll.evaluate((element) => element.scrollTo(0, element.scrollHeight));
    await expect(chart.getByText(/400 of 20,001 owners loaded/)).toBeVisible();
    await scroll.evaluate((element) => element.scrollTo(0, 199 * 48));
    for (const date of ['2026-01-01', '2026-02-01']) {
      const name = scroll.getByTitle(`Boundary Alex · Joined ${date}`, { exact: true });
      await expect(name).toBeVisible();
      await name.focus();
      await fullTooltip(
        page.getByRole('tooltip').filter({ hasText: `Boundary Alex · Joined ${date}` }),
      );
    }
    await page.screenshot({ path: info.outputPath(`cross-page-duplicates-${theme}.png`) });
    // The eighth visible row after scrolling sits directly above the fixed axis.
    await scroll.evaluate((element) => element.scrollTo(0, 200 * 48));
    await scroll.getByTitle(longName, { exact: true }).focus();
    await fullTooltip(page.getByRole('tooltip').filter({ hasText: longName }));
    await page.screenshot({ path: info.outputPath(`bottom-row-focus-${theme}.png`) });
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
    // Focus the final row via Tab, including its multi-line disclosure.
    await scroll.getByTitle('Alexandra Montgomery Research Operations', { exact: true }).focus();
    await page.keyboard.press('Tab');
    await expect(scroll.getByTitle(longName, { exact: true })).toBeFocused();
    const tooltip = page.getByRole('tooltip').filter({ hasText: longName });
    await fullTooltip(tooltip);
    await page.screenshot({ path: info.outputPath(`long-name-focus-${theme}.png`) });
    await page.keyboard.press('Escape');
    await expect(tooltip).toHaveCount(0);
    expect(metrics.creates).toHaveLength(creates);
    expect(boundary.writes).toEqual([]);
    expect(errors).toEqual([]);
  });
}
