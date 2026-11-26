import { test, expect } from '@playwright/test';
import { registerUsageRedesignCases } from './redesign-usage.cases';

import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { LocalUsageFilterOptions } from './usage-filter-options.fixture';
import { asOf, metricRow } from '../src/usage/metrics/fixtures.test-support';

registerUsageRedesignCases();

for (const admin of [false, true]) {
  test(`LOCAL allocated sorting keeps API pagination and confirmed missing intervals for admin=${admin}`, async ({
    page,
    baseURL,
  }) => {
    const boundary = new LocalStartup();
    const metrics = new LocalUsageMetrics();
    const values = [8.165, 14.729, 37.688, 47.935, 0, 8.165, 8.1654];
    const rows = Array.from({ length: 61 }, (_, n) => ({
      ...metricRow,
      sandbox_id: `sort-${String(n).padStart(3, '0')}`,
      name: `Subtotal ${String(n).padStart(3, '0')}`,
      allocated_period: {
        ...metricRow.allocated_period,
        cpu_core_hours: values[n % 7]!,
        memory_gib_hours: values[(n * 3 + 1) % 7]!,
        disk_gib_hours: values[(n * 5 + 2) % 7]!,
        complete: n % 2 === 0,
        unknown_seconds: n % 2 === 0 ? 0 : 600,
      },
    }));
    metrics.presentationRows = rows;
    await boundary.route(page, baseURL!);
    await metrics.route(page);
    await metrics.session(
      page,
      admin ? ['use_own_sandboxes', 'read_all_sandboxes'] : ['use_own_sandboxes'],
    );
    const reads: URL[] = [];
    let delivered = rows.slice(0, 50);
    await page.route(/\/usage\/metrics\/reports\/[^/]+\/sandboxes\?/, async (route) => {
      const url = new URL(route.request().url());
      reads.push(url);
      const reportId = url.pathname.split('/reports/')[1]!.split('/')[0]!;
      const sort = url.searchParams.get('sort')!;
      const order = url.searchParams.get('order')!;
      const field =
        sort === 'allocated_cpu_core_hours'
          ? 'cpu_core_hours'
          : sort === 'allocated_memory_gib_hours'
            ? 'memory_gib_hours'
            : 'disk_gib_hours';
      const ordered = [...rows].sort(
        (a, b) =>
          (order === 'asc' ? 1 : -1) *
          (a.allocated_period[field] - b.allocated_period[field] ||
            a.sandbox_id.localeCompare(b.sandbox_id)),
      );
      const cursor = url.searchParams.get('cursor');
      if (cursor) expect(cursor).toBe(`${reportId}:${sort}:${order}:50`);
      expect(url.searchParams.get('limit')).toBe('50');
      const offset = cursor ? 50 : 0;
      delivered = ordered.slice(offset, offset + 50);
      await route.fulfill({
        json: {
          report_id: reportId,
          as_of: metrics.reports.get(reportId)!.as_of,
          items: delivered,
          total_count: rows.length,
          next: offset === 0 ? `${reportId}:${sort}:${order}:50` : null,
        },
      });
    });
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto(admin ? '/operations?tab=sandboxes' : '/usage');
    const surface = admin
      ? page.getByRole('region', { name: 'Sandbox resource dashboard' })
      : page.getByRole('dialog', { name: 'Your usage' });
    const table = surface.getByRole('table', { name: 'Sandboxes and allocated usage' });
    const assertPage = async (sort: string, order: string, second: boolean) => {
      await expect.poll(() => reads.at(-1)?.searchParams.get('sort')).toBe(sort);
      await expect.poll(() => reads.at(-1)?.searchParams.get('order')).toBe(order);
      await expect.poll(() => reads.at(-1)?.searchParams.has('cursor')).toBe(second);
      await expect(table).not.toHaveAttribute('aria-busy', 'true');
      await expect(table.getByRole('button', { name: /^Subtotal / })).toHaveText(
        delivered.map((row) => row.name),
      );
      await expect(table.getByText('Missing intervals confirmed', { exact: true })).toHaveCount(
        delivered.filter((row) => !row.allocated_period.complete).length,
      );
      return delivered.map((row) => row.sandbox_id);
    };
    await expect(table.getByRole('button', { name: /^Subtotal / })).toHaveCount(50);
    for (const [sort, label] of [
      ['allocated_cpu_core_hours', 'CPU (core-hours)'],
      ['allocated_memory_gib_hours', 'Memory (GiB-hours)'],
      ['allocated_disk_gib_hours', 'Disk (GiB-hours)'],
    ]) {
      await table.getByRole('columnheader').filter({ hasText: label }).getByRole('button').click();
      for (const order of ['asc', 'desc']) {
        const first = await assertPage(sort!, order, false);
        await surface.getByRole('button', { name: 'Next usage page' }).click();
        const last = await assertPage(sort!, order, true);
        expect(new Set([...first, ...last]).size).toBe(61);
        await expect(surface.getByRole('button', { name: 'Next usage page' })).toBeDisabled();
        if (order === 'asc')
          await surface.getByRole('button', { name: 'Sort descending', exact: true }).click();
      }
    }
    const oldReport = metrics.creates.at(-1)!.id;
    await surface.getByRole('button', { name: 'Refresh usage', exact: true }).click();
    await expect.poll(() => metrics.creates.at(-1)!.id).not.toBe(oldReport);
    await assertPage('allocated_disk_gib_hours', 'desc', false);
    expect(reads.at(-1)!.pathname).toContain(metrics.creates.at(-1)!.id);
    expect(reads.every((url) => url.pathname.includes('/admin/') === admin)).toBe(true);
    expect(boundary.writes).toEqual([]);
    expect(boundary.errors).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
  });
}

for (const admin of [false, true]) {
  test(`LOCAL allocation coverage follows refresh and filters in ${admin ? 'operations' : 'personal usage'}`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = new LocalStartup();
    const metrics = new LocalUsageMetrics();
    const options = new LocalUsageFilterOptions();
    let daily = {
      ...metricRow.allocated_period,
      complete: false,
      unknown_seconds: 0,
      projected_tail_seconds: 173040,
    };
    metrics.allocatedPeriod = (query) =>
      query.filters?.sandbox_id
        ? { ...daily, complete: true, unknown_seconds: 0, projected_tail_seconds: 0 }
        : query.to! - query.from! > 86400
          ? { ...daily, unknown_seconds: 0, projected_tail_seconds: 0 }
          : daily;
    await boundary.route(page, baseURL!);
    await metrics.route(page);
    await metrics.session(
      page,
      admin ? ['use_own_sandboxes', 'read_all_sandboxes'] : ['use_own_sandboxes'],
    );
    if (admin) await options.route(page);
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.goto(admin ? '/operations?tab=sandboxes' : '/usage');
    const surface = admin
      ? page.getByRole('region', { name: 'Sandbox resource dashboard' })
      : page.getByRole('dialog', { name: 'Your usage' });
    const history = surface
      .getByRole('region', { name: 'Allocated resource-hours', exact: true })
      .locator('.usage-allocation-history');
    const assertHistory = async (
      badge: string,
      summary: string,
      missing: string,
      projected: string,
    ) => {
      await expect(history.locator('.badge')).toHaveText(badge);
      await expect(history.locator('summary')).toContainText(summary);
      if ((await history.getAttribute('open')) === null) await history.locator('summary').click();
      for (const [label, duration] of [
        ['Missing intervals', missing],
        [
          'Projection from last known state',
          projected === 'None' ? 'None' : `${projected} (estimated)`,
        ],
      ])
        await expect(
          history.locator('dt').filter({ hasText: label }).locator('..').locator('dd'),
        ).toHaveText(duration!);
      await expect(history.getByText('How this is calculated')).toBeVisible();
      await expect(history.getByText(/No missing interval has been established/)).toHaveCount(
        badge === 'Coverage unverified' ? 1 : 0,
      );
      await expect(history.getByText(/Durations add up across Sandboxes/)).toBeVisible();
    };
    await expect(surface.getByRole('button', { name: 'Usage time range' })).toContainText(
      'Last 24 hours',
    );
    await expect(
      surface.getByRole('button', {
        name: admin ? 'Sandbox: All Sandboxes' : 'Sandbox: All my Sandboxes',
      }),
    ).toBeVisible();
    await assertHistory(
      'Coverage unverified',
      'Projection from last known state',
      'None confirmed',
      '2d 4m',
    );
    await page.screenshot({ path: info.outputPath('projected-history.png') });
    await page.reload();
    await assertHistory(
      'Coverage unverified',
      'Projection from last known state',
      'None confirmed',
      '2d 4m',
    );
    daily = { ...daily, unknown_seconds: 3600, projected_tail_seconds: 120 };
    await surface.getByRole('button', { name: 'Refresh usage', exact: true }).click();
    await assertHistory('Missing intervals confirmed', 'Confirmed missing duration', '1h', '2m');
    await expect(
      history.getByText(/Missing intervals are excluded.*does not mean zero/),
    ).toBeVisible();
    await surface.getByRole('button', { name: 'Usage time range' }).click();
    await page.getByRole('menuitemradio', { name: 'Last 7 days', exact: true }).click();
    await surface.getByRole('button', { name: 'Apply usage filters' }).click();
    await assertHistory(
      'Coverage unverified',
      'No missing interval has been established',
      'None confirmed',
      'None',
    );
    expect(metrics.creates.at(-1)!.input.to! - metrics.creates.at(-1)!.input.from!).toBe(604800);
    await surface
      .getByRole('button', { name: admin ? 'Sandbox: All Sandboxes' : 'Sandbox: All my Sandboxes' })
      .click();
    if (!admin) await surface.getByRole('button', { name: 'Choose sandbox' }).click();
    await page
      .getByRole('menuitemradio', { name: admin ? 'Research 00000' : /^Research Sandbox \(/ })
      .click();
    await surface.getByRole('button', { name: 'Apply usage filters' }).click();
    await assertHistory('Coverage verified', 'How this is calculated', 'None', 'None');
    expect(metrics.creates.at(-1)!.input.filters?.sandbox_id).toBe(
      admin ? options.rows[0]!.value : metricRow.sandbox_id,
    );
    metrics.allocatedPeriod = () => ({
      ...daily,
      complete: true,
      unknown_seconds: 0,
      projected_tail_seconds: 60,
    });
    await surface.getByRole('button', { name: 'Refresh usage', exact: true }).click();
    await assertHistory('Coverage verified', 'Projection from last known state', 'None', '1m');
    expect(metrics.creates.every((read) => read.admin === admin)).toBe(true);
    if (!admin) expect(metrics.creates.every((read) => !read.input.filters?.owner)).toBe(true);
    expect(boundary.writes).toEqual([]);
    expect(boundary.errors).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
  });

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
  const search = surface.getByRole('searchbox', { name: 'Sandbox name or ID starts with' });
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
