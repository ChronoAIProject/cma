import { expect, test } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { seriesFixture } from '../src/usage/metrics/fixtures.test-support';

for (const state of ['disabled', 'missing', 'partial', 'unavailable', 'settled'] as const) {
  test(`LOCAL TC-105 ${state}: usage preserves the distinction between allocation and measurements`, async ({
    page,
    baseURL,
  }) => {
    const boundary = new LocalStartup();
    const metrics = new LocalUsageMetrics();
    metrics.measuredVisible = state !== 'disabled';
    await boundary.route(page, baseURL!);
    await metrics.route(page);
    await metrics.session(page);
    let chartRequests = 0;
    let unavailable = state === 'unavailable';
    await page.route('**/api/v1/usage/metrics/reports/*/series?*', async (route) => {
      const url = new URL(route.request().url());
      const metric = url.searchParams.get('metric');
      expect(metric).toBe('cpu');
      expect(route.request().method()).toBe('GET');
      chartRequests++;
      if (unavailable) {
        await route.fulfill({
          status: 503,
          json: { status: 503, title: 'Usage unavailable', code: 'usage_unavailable' },
        });
        return;
      }
      const reportId = url.pathname.split('/').at(-2);
      const series = seriesFixture('cpu', reportId);
      const line = series.lines[0]!;
      const [observed, missing] = line.points;
      const settled = {
        ...observed!,
        covered_seconds: observed!.expected_seconds,
        reason: null,
        excluded_count: 0,
      };
      const points =
        state === 'missing' ? [missing!] : state === 'partial' ? line.points : [settled];
      await route.fulfill({ json: { ...series, lines: [{ ...line, points }] } });
    });

    await page.goto('/usage');
    const dialog = page.getByRole('dialog', { name: 'Your usage' });
    const allocated = dialog.getByRole('region', { name: 'Allocated resource-hours' });
    await expect(allocated).toContainText(
      'Resource-hours describe allocation; billing is separate',
    );
    await expect(allocated).toContainText('480');
    if (state === 'disabled') {
      await expect(dialog.getByRole('region', { name: 'Measured resource charts' })).toHaveCount(0);
      expect(chartRequests).toBe(0);
    } else {
      await dialog.getByRole('button', { name: 'CPU', exact: true }).click();
      const charts = dialog.getByRole('region', { name: 'Measured resource charts' });
      if (state === 'unavailable') {
        await expect(charts.getByRole('alert')).toBeVisible({ timeout: 15_000 });
        await expect(charts.getByRole('region', { name: 'CPU chart' })).toHaveCount(0);
        unavailable = false;
        await charts.getByRole('button', { name: 'Retry usage', exact: true }).click();
      }
      const chart = charts.getByRole('region', { name: 'CPU chart' });
      await expect(
        chart.getByText(
          'Source: Fixture runtime measurements. Gaps indicate unavailable measurements.',
        ),
      ).toBeVisible();
      await expect(charts.getByRole('alert')).toHaveCount(0);
      await chart.getByText('View CPU chart data', { exact: true }).click();
      const table = chart.getByRole('table', { name: 'CPU chart data' });
      if (state === 'missing') {
        await expect(
          chart.getByText('No measured samples in this range.', { exact: true }),
        ).toBeVisible();
        await expect(table.getByRole('cell', { name: '0 cores', exact: true })).toHaveCount(0);
      } else {
        await expect(table.getByRole('cell', { name: '0.5 cores', exact: true })).toBeVisible();
      }
      if (state === 'missing' || state === 'partial') {
        await expect(table.getByRole('cell', { name: /no measurement/ })).toBeVisible();
      } else {
        await expect(table.getByRole('cell', { name: 'Observed; 0 excluded' })).toBeVisible();
      }
    }
    expect(
      metrics.reads.every((url) => url.origin === baseURL && !url.pathname.includes('/admin/')),
    ).toBe(true);
    expect(boundary.writes).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}
