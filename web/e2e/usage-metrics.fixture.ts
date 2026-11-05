import type { Page } from '@playwright/test';
import type { UsageMetricQuery, UsageMetricReport } from '@chronoai/cma-sdk';
import {
  asOf,
  metricRow,
  reportFixture,
  seriesFixture,
} from '../src/usage/metrics/fixtures.test-support';

/** Synthetic cohort: the browser sees 50-row requests, never a full inventory scan. */
export class LocalUsageMetrics {
  readonly reads: URL[] = [];
  readonly creates: { admin: boolean; input: UsageMetricQuery; id: string }[] = [];
  readonly reports = new Map<string, UsageMetricReport>();
  failure = 0;
  pending = false;
  async route(page: Page) {
    await page.route('**/api/v1/**usage/metrics/reports**', async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      this.reads.push(url);
      const admin = url.pathname.includes('/admin/');
      if (this.failure)
        return route.fulfill({
          status: this.failure,
          json: {
            status: this.failure,
            title: 'Usage unavailable',
            detail: 'Fixture reporting unavailable',
            code: 'usage_unavailable',
          },
        });
      if (request.method() === 'POST') {
        const input = request.postDataJSON() as UsageMetricQuery;
        const id = `report-${this.creates.length + 1}`;
        this.creates.push({ admin, input, id });
        const base = reportFixture(admin, id);
        const report: UsageMetricReport = {
          ...base,
          query: {
            from: input.from ?? asOf - 86400,
            to: input.to ?? asOf,
            step_seconds: input.step_seconds ?? 60,
            filters: {
              owner: input.filters?.owner ?? null,
              statuses: input.filters?.statuses ?? [],
              runtime_version: input.filters?.runtime_version ?? null,
              sandbox_id: input.filters?.sandbox_id ?? null,
              agent_id: input.filters?.agent_id ?? null,
              search: input.filters?.search ?? '',
            },
          },
          summary: {
            ...base.summary!,
            sandbox_count: input.filters?.search === 'empty' ? 0 : 25001,
            running_count: input.filters?.search === 'empty' ? 0 : 18000,
          },
        };
        this.reports.set(id, report);
        return route.fulfill({
          status: this.pending ? 202 : 200,
          json: this.pending ? { ...report, status: 'pending', summary: null } : report,
        });
      }
      const suffix = url.pathname.split('/reports/')[1] ?? '';
      const [id = '', member, resource] = suffix.split('/');
      const report = this.reports.get(id);
      if (!report)
        return route.fulfill({
          status: 410,
          json: {
            status: 410,
            title: 'Report expired',
            detail: 'Fixture report expired',
            code: 'report_expired',
          },
        });
      if (!member) return route.fulfill({ json: report });
      if (member === 'owners')
        return route.fulfill({
          json: {
            report_id: id,
            items: [
              {
                subject: url.searchParams.get('cursor') ? 'owner-25001' : 'owner-1',
                display_name: url.searchParams.get('cursor') ? 'Remote owner' : 'Alex Chen',
              },
            ],
            next: url.searchParams.get('cursor') ? null : 'owners-next',
          },
        });
      if (member === 'sandboxes') {
        if (resource) return route.fulfill({ json: { ...metricRow, sandbox_id: resource } });
        const second = url.searchParams.has('cursor');
        return route.fulfill({
          json: {
            report_id: id,
            as_of: asOf,
            items:
              report.query.filters.search === 'empty'
                ? []
                : [
                    {
                      ...metricRow,
                      ...(second ? { sandbox_id: 'sbx-2', name: 'Second page Sandbox' } : {}),
                    },
                  ],
            next: second ? null : 'page-two',
            total_count: report.summary?.sandbox_count,
          },
        });
      }
      if (member === 'series') {
        const metric = url.searchParams.get('metric') as 'cpu' | 'memory' | 'disk';
        const series = seriesFixture(metric, id);
        const line = series.lines[0]!;
        const first = line.points[0]!;
        const points = [0, 1, 2, 3, 4, 5].map((index) => ({
          ...first,
          from: asOf - 360 + index * 60,
          to: asOf - 300 + index * 60,
          mean: index === 2 ? null : Number(first.mean) * (1 + index / 8),
          reason: index === 2 ? 'no_measurement' : 'partial_coverage',
        }));
        return route.fulfill({
          json: {
            ...series,
            lines:
              report.query.filters.search === 'empty'
                ? []
                : [
                    { ...line, points },
                    ...(url.searchParams.get('group_by') === 'owner'
                      ? [{ ...line, id: 'other', label: 'Other', points }]
                      : []),
                  ],
          },
        });
      }
      return route.fulfill({ status: 404, json: {} });
    });
    await page.route('**/api/v1/usage/sandboxes/*/events**', (route) =>
      route.fulfill({ json: { items: [], next: null } }),
    );
  }
}
