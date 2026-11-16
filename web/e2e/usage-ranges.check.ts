import { test, expect, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { LocalUsageFilterOptions } from './usage-filter-options.fixture';
import { asOf, metricRow, reportFixture } from '../src/usage/metrics/fixtures.test-support';

/** Public HTTP fixtures: real compiled browser and SDK; no real generation latency. */
async function fixture(page: Page, baseURL: string) {
  const boundary = new LocalStartup();
  const metrics = new LocalUsageMetrics();
  await boundary.route(page, baseURL);
  await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
  await metrics.route(page);
  await new LocalUsageFilterOptions().route(page);
  const calls: { route: string; window: string; at: number }[] = [];
  const state = { month: 'pending', sevenFailures: 0, descriptorStatus: 0 };
  const snapshot = (window: string) => {
    const seconds = window === '7d' ? 604800 : window === '30d' ? 2592000 : 86400;
    const base = reportFixture(true, `range-${window}`);
    return {
      ...base,
      scope: { principal: 'local-owner', admin: true },
      query: {
        ...base.query,
        from: asOf - seconds,
        to: asOf,
        step_seconds: window === '7d' ? 600 : window === '30d' ? 3600 : 60,
      },
      effective_range: { from: asOf - seconds, to: asOf },
      summary: { ...base.summary!, sandbox_count: 50, running_count: 50 },
    };
  };
  await page.route('**/api/v1/admin/usage/metrics/view**', async (route) => {
    const url = new URL(route.request().url());
    const window = url.searchParams.get('window')!;
    calls.push({ route: 'view', window, at: Date.now() });
    expect(url.searchParams.has('step_seconds')).toBe(false);
    expect(url.searchParams.has('from')).toBe(false);
    expect(url.searchParams.has('to')).toBe(false);
    if (window === '7d' && state.sevenFailures-- > 0)
      return route.fulfill({
        status: 503,
        headers: { 'Retry-After': '2' },
        json: { status: 503, title: 'Temporarily unavailable', code: 'usage_unavailable' },
      });
    const pending = window === '30d' && state.month === 'pending';
    const report = snapshot(window);
    return route.fulfill({
      status: pending ? 202 : 200,
      json: {
        schema_version: 1,
        window,
        requested_range: report.effective_range,
        report: pending
          ? { ...report, status: 'pending', summary: null, report_complete: false }
          : report,
        generated_at: pending ? null : asOf,
        stale: false,
        refresh: {
          state: pending ? 'pending' : 'idle',
          report_id: pending ? report.report_id : null,
          retry_after_seconds: 2,
        },
      },
    });
  });
  await page.route('**/api/v1/admin/usage/metrics/reports/range-**', async (route) => {
    const suffix = new URL(route.request().url()).pathname.split('/reports/range-')[1]!;
    const [window, child] = suffix.split('/');
    calls.push({ route: child ?? 'descriptor', window: window!, at: Date.now() });
    const report = snapshot(window!);
    if (child === 'sandboxes')
      return route.fulfill({
        json: {
          report_id: report.report_id,
          as_of: asOf,
          total_count: 50,
          next: null,
          items: Array.from({ length: 50 }, (_, index) => ({
            ...metricRow,
            sandbox_id: `sandbox-${index}`,
            name: `Fixture Sandbox ${index}`,
            owner: { subject: `owner-${index}`, display_name: `Fixture Owner ${index}` },
          })),
        },
      });
    if (state.descriptorStatus)
      return route.fulfill({
        status: state.descriptorStatus,
        json: { status: state.descriptorStatus, title: 'Expired', code: 'report_expired' },
      });
    const pending = window === '30d' && state.month === 'pending';
    return route.fulfill({
      json: pending
        ? {
            ...report,
            status: 'pending',
            summary: null,
            report_complete: false,
            retry_after_seconds: 2,
          }
        : report,
    });
  });
  return { state, calls, boundary };
}
async function select(page: Page, range: '7' | '30') {
  await page.getByRole('button', { name: 'Usage time range' }).click();
  await page.getByRole('menuitemradio', { name: `Last ${range} days`, exact: true }).click();
  await page.getByRole('button', { name: 'Apply usage filters' }).click();
}
async function complete(page: Page, range: string) {
  const dashboard = page.locator('.usage-metrics');
  await expect(dashboard).toHaveAttribute('data-usage-requested-range', range);
  await expect(dashboard).toHaveAttribute('data-usage-state', 'committed');
  await expect(page.getByRole('button', { name: /^Fixture Sandbox / })).toHaveCount(50);
  await expect(page.getByText(/^Fixture Owner /)).toHaveCount(50);
  await expect(page.getByRole('region', { name: 'Allocation comparison chart' })).toBeVisible();
  await page.evaluate(
    () =>
      new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      ),
  );
}

test('LOCAL compiled range switching uses labelled previews and the warmed immutable ID', async ({
  page,
  baseURL,
}, info) => {
  const { calls, state, boundary } = await fixture(page, baseURL!);
  await page.goto('/operations?tab=sandboxes');
  await complete(page, '24h');
  await expect
    .poll(() => calls.filter((row) => row.route === 'view').map((row) => row.window))
    .toEqual(['24h', '7d', '30d']);
  await select(page, '7');
  await complete(page, '7d');
  expect(calls.filter((row) => row.route === 'view')).toHaveLength(3);
  await select(page, '30');
  await expect(page.getByText('Showing Last 7 days', { exact: true })).toBeVisible();
  await expect(page.locator('.usage-metrics')).toHaveAttribute('data-usage-state', 'preview');
  await expect(page.getByText(/not last 30 days/)).toBeVisible();
  await expect(page.locator('.usage-report')).not.toContainText('selected period');
  await expect(page.locator('.usage-report')).toContainText(
    'Capacity reserved over the report period.',
  );
  await expect(page.getByText(/completion estimate is not available/)).toBeVisible();
  await expect(page.locator('.usage-loading')).toHaveCount(0);
  await page.screenshot({ path: info.outputPath('range-preview.png'), fullPage: true });
  state.month = 'ready';
  await complete(page, '30d');
  expect(
    calls.filter((row) => row.route === 'descriptor').every((row) => row.window === '30d'),
  ).toBe(true);
  expect(calls.filter((row) => row.route === 'view' && row.window === '30d')).toHaveLength(2);
  expect(boundary.errors).toEqual([]);
  expect(boundary.writes).toEqual([]);
});

test('LOCAL compiled seven-day failure automatically recovers after Retry-After', async ({
  page,
  baseURL,
}) => {
  const { calls, state } = await fixture(page, baseURL!);
  state.sevenFailures = 1;
  await page.goto('/operations?tab=sandboxes&range=7d');
  await expect(page.getByText('Retrying usage automatically…', { exact: true })).toBeVisible();
  await expect(page.getByRole('alert')).toHaveCount(0);
  await complete(page, '7d');
  const reads = calls.filter((row) => row.route === 'view' && row.window === '7d');
  expect(reads).toHaveLength(2);
  expect(reads[1]!.at - reads[0]!.at).toBeGreaterThanOrEqual(2000);
});

test('LOCAL compiled thirty-day timeout stops with useful progress and no re-admission', async ({
  page,
  baseURL,
}) => {
  const { calls } = await fixture(page, baseURL!);
  await page.clock.install();
  await page.goto('/operations?tab=sandboxes&range=30d');
  await expect(page.getByText(/Last 30 days: waiting for the complete report/)).toBeVisible();
  await expect(page.locator('.usage-loading')).toHaveCount(0);
  await page.clock.fastForward(121000);
  await expect(
    page.getByText('This report is taking longer than expected', { exact: true }),
  ).toBeVisible();
  const observations = calls.filter((row) => row.route === 'descriptor').length;
  await page.clock.fastForward(121000);
  expect(calls.filter((row) => row.route === 'descriptor')).toHaveLength(observations);
  expect(calls.filter((row) => row.route === 'view' && row.window === '30d')).toHaveLength(1);
  await expect(page.getByRole('button', { name: 'Retry usage' })).toBeEnabled();
});
