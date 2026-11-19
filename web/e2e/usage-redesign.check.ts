import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { asOf, metricRow } from '../src/usage/metrics/fixtures.test-support';

for (const admin of [false, true]) {
  for (const theme of ['light', 'dark']) {
    for (const width of [1440, 390]) {
      test(`LOCAL allocated redesign ${admin ? 'operations' : 'personal'} ${theme} ${width}`, async ({
        page,
        baseURL,
      }, info) => {
        const boundary = new LocalStartup();
        const metrics = new LocalUsageMetrics();
        metrics.presentationRows = [
          {
            ...metricRow,
            sandbox_id: 'sbx_fixture_research_0123456789',
            name: 'Research workspace',
            owner: { subject: 'fixture-owner-a', display_name: 'Fixture owner A' },
            runtime_version: `registry.example.test/cma/runtime:stable@sha256:${'a'.repeat(64)}`,
            allocated_period: {
              ...metricRow.allocated_period,
              cpu_core_hours: 36,
              memory_gib_hours: 72,
              disk_gib_hours: 480,
              unknown_seconds: 5400,
              projected_tail_seconds: 120,
            },
          },
          {
            ...metricRow,
            sandbox_id: 'sbx_fixture_build_0123456789',
            name: 'Build and test',
            owner: { subject: 'fixture-owner-a', display_name: 'Fixture owner A' },
            runtime_version: 'runtime:stable',
            allocated_period: {
              ...metricRow.allocated_period,
              cpu_core_hours: 18,
              memory_gib_hours: 36,
              disk_gib_hours: 240,
              complete: true,
              unknown_seconds: 0,
              projected_tail_seconds: 60,
            },
          },
          {
            ...metricRow,
            sandbox_id: 'sbx_fixture_review_0123456789',
            name: 'Review workspace',
            owner: { subject: 'fixture-owner-b', display_name: 'Fixture owner B' },
            status: 'paused',
            allocation: { cpu_cores: 0, memory_bytes: 0, disk_bytes: 20 * 2 ** 30 },
            allocated_period: {
              ...metricRow.allocated_period,
              cpu_core_hours: 8,
              memory_gib_hours: 16,
              disk_gib_hours: 480,
              complete: true,
              unknown_seconds: 0,
              projected_tail_seconds: 30,
            },
          },
          {
            ...metricRow,
            sandbox_id: 'sbx_fixture_archive_0123456789',
            name: 'Documentation',
            owner: { subject: 'fixture-owner-b', display_name: 'Fixture owner B' },
            status: 'paused',
            allocation: { cpu_cores: 0, memory_bytes: 0, disk_bytes: 10 * 2 ** 30 },
            allocated_period: {
              ...metricRow.allocated_period,
              cpu_core_hours: 4,
              memory_gib_hours: 8,
              disk_gib_hours: 120,
              complete: true,
              unknown_seconds: 0,
              projected_tail_seconds: 0,
            },
          },
        ];
        await boundary.route(page, baseURL!);
        await metrics.route(page);
        await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
        await page.addInitScript(
          (appearance) => localStorage.setItem('cma:appearance', appearance),
          theme,
        );
        await page.setViewportSize({ width, height: 1000 });
        await page.goto(admin ? '/operations?tab=sandboxes' : '/usage');
        const surface = admin
          ? page.getByRole('region', { name: 'Sandbox resource dashboard' })
          : page.getByRole('dialog', { name: 'Your usage' });
        const chart = surface.getByRole('region', { name: 'Allocation comparison chart' });
        await expect(chart.locator('.recharts-bar-rectangle')).toHaveCount(4);
        await expect(surface.getByRole('region', { name: 'Measured resource charts' })).toHaveCount(
          0,
        );
        await expect(surface.getByRole('button', { name: 'Apply usage filters' })).toHaveCSS(
          'border-radius',
          '10px',
        );
        await expect(surface.getByRole('button', { name: 'Apply usage filters' })).not.toHaveCSS(
          'background-color',
          'rgba(0, 0, 0, 0)',
        );
        const filters = surface.getByRole('form', { name: 'Usage filters' });
        const controls = filters.locator(
          '.usage-filter-control, .usage-discovery-control, .usage-filter-apply',
        );
        await expect
          .poll(() =>
            controls.evaluateAll((nodes) =>
              nodes.every((node) => Math.abs(node.getBoundingClientRect().height - 44) < 1),
            ),
          )
          .toBe(true);
        if (width === 1440)
          await expect
            .poll(() =>
              controls.evaluateAll(
                (nodes) =>
                  new Set(nodes.map((node) => Math.round(node.getBoundingClientRect().y))).size,
              ),
            )
            .toBe(1);
        const refresh = surface.getByRole('button', { name: 'Refresh usage', exact: true });
        const heading = surface.getByRole('heading', { name: 'Allocation overview' });
        if (width === 1440)
          await expect
            .poll(async () =>
              Math.abs((await refresh.boundingBox())!.y - (await heading.boundingBox())!.y),
            )
            .toBeLessThan(2);
        await expect(
          chart.locator('text > title').filter({ hasText: 'Research workspace' }),
        ).toHaveCount(1);
        const scroll = admin ? page.locator('.operator-scroll') : surface.locator('.modal__body');
        await scroll.evaluate((node) => node.scrollTo(0, 0));
        await page.screenshot({ path: info.outputPath('overview.png'), animations: 'disabled' });
        await chart.evaluate((node) => node.scrollIntoView({ block: 'start' }));
        await expect(chart.locator('.recharts-bar-rectangle').first()).toBeVisible();
        await page.screenshot({ path: info.outputPath('chart.png'), animations: 'disabled' });
        const table = surface.getByRole('table', { name: 'Sandboxes and allocated usage' });
        await surface
          .locator('.usage-inventory > .usage-section-heading')
          .evaluate((node) => node.scrollIntoView({ block: 'start' }));
        expect(await table.evaluate((node) => node.scrollWidth <= node.clientWidth + 1)).toBe(true);
        expect(await scroll.evaluate((node) => node.scrollWidth <= node.clientWidth + 1)).toBe(
          true,
        );
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
        ).toBe(true);
        await page.screenshot({ path: info.outputPath('inventory.png'), animations: 'disabled' });
        await surface.getByText('View allocation chart data', { exact: true }).focus();
        await page.keyboard.press('Enter');
        await expect(surface.getByRole('table', { name: 'Allocation chart data' })).toBeVisible();
        await table.locator('summary').first().click();
        await expect(
          surface.getByRole('button', { name: 'Copy runtime image' }).first(),
        ).toBeVisible();
        expect(await scroll.evaluate((node) => node.scrollWidth <= node.clientWidth + 1)).toBe(
          true,
        );
        expect(metrics.reads.filter((url) => url.pathname.endsWith('/series'))).toHaveLength(0);
        expect(boundary.writes).toEqual([]);
        expect(boundary.errors).toEqual([]);
      });
    }
  }
}

test('LOCAL Usage count copy follows applied searches through one, zero and multiple results', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  const metrics = new LocalUsageMetrics();
  const rows = [
    { ...metricRow, status: 'paused' as const },
    { ...metricRow, sandbox_id: 'sbx-build', name: 'Build Sandbox', status: 'paused' as const },
  ];
  const matching = (search = '') =>
    rows.filter((row) => row.name.toLowerCase().includes(search.toLowerCase()));
  metrics.summaryCount = (query) => matching(query.filters?.search).length;
  metrics.runningCount = () => 0;
  await boundary.route(page, baseURL!);
  await metrics.route(page);
  await metrics.session(page);
  await page.route('**/api/v1/usage/metrics/reports/*/sandboxes**', async (route) => {
    const url = new URL(route.request().url());
    const id = url.pathname.split('/reports/')[1]!.split('/')[0]!;
    const report = metrics.reports.get(id)!;
    expect(route.request().method()).toBe('GET');
    expect(url.searchParams.get('limit')).toBe('50');
    expect(url.searchParams.has('cursor')).toBe(false);
    metrics.reads.push(url);
    await route.fulfill({
      json: {
        report_id: id,
        as_of: asOf,
        items: matching(report.query.filters.search),
        total_count: report.summary!.sandbox_count,
        next: null,
      },
    });
  });
  await page.goto('/usage');
  const surface = page.getByRole('dialog', { name: 'Your usage' });
  const search = surface.getByRole('searchbox', { name: 'Usage Sandbox search' });
  const chart = surface.getByRole('region', { name: 'Allocation comparison chart' });
  await expect(surface.getByText('2 Sandboxes · 0 running', { exact: true })).toBeVisible();
  for (const [query, count, label] of [
    ['Research', 1, '1 Sandbox'],
    ['', 2, '2 Sandboxes'],
    ['empty', 0, '0 Sandboxes'],
    ['Research', 1, '1 Sandbox'],
  ] as const) {
    await search.fill(query);
    await surface.getByRole('button', { name: 'Apply usage filters' }).click();
    await expect(surface.getByText(`${label} · 0 running`, { exact: true })).toBeVisible();
    await expect(
      surface.getByText(`${label} · 50 per page · page 1`, { exact: true }),
    ).toBeVisible();
    await expect(surface.getByRole('heading', { name: 'Sandboxes', exact: true })).toBeVisible();
    if (count === 0) {
      await expect(chart).toHaveCount(0);
      await expect(surface.getByText('No Sandboxes match this selection.')).toBeVisible();
    } else {
      await expect(
        chart.getByText(`${count} of ${label} on this page.`, { exact: true }),
      ).toBeVisible();
      await expect(chart.locator('.recharts-bar-rectangle')).toHaveCount(count);
    }
    await expect(surface.getByRole('button', { name: 'Next usage page' })).toBeDisabled();
    expect(new URL(page.url()).searchParams.get('search') ?? '').toBe(query);
  }
  expect(metrics.creates.every(({ admin, input }) => !admin && !input.filters?.owner)).toBe(true);
  expect(metrics.reads.filter((url) => url.pathname.endsWith('/series'))).toHaveLength(0);
  expect(boundary.writes).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
