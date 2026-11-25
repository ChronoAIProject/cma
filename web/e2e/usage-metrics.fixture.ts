import type { Page } from '@playwright/test';
import type {
  UsageMetricQuery,
  UsageMetricReport,
  UsageMetricRow,
  UsageOwnerAllocation,
} from '@cma/client';
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
  presentationRows?: UsageMetricRow[];
  allocatedPeriod?: (query: UsageMetricQuery) => UsageMetricRow['allocated_period'];
  summaryCount?: (query: UsageMetricQuery) => number;
  runningCount?: (query: UsageMetricQuery) => number;
  ownerCount = 400;
  readonly ownerOverrides = new Map<number, Partial<UsageOwnerAllocation>>();
  ownerFailure = 0;
  failure = 0;
  retryAfterSeconds?: number;
  pending = false;
  /** Read-only session exposure; the test may change it after an admin save. */
  measuredVisible = false;
  async session(page: Page, capabilities = ['use_own_sandboxes']) {
    await page.route('**/auth/session', (route) =>
      route.fulfill({
        json: {
          authenticated: true,
          subject: 'local-owner',
          display_name: 'Local operator',
          visitor: false,
          capabilities,
          usage_reporting: { measured_visible: this.measuredVisible },
        },
      }),
    );
  }
  async route(page: Page) {
    // The shared browser fixtures model a replica that predates the bounded
    // initial-view routes. Use the typed route-absence response so the client
    // exercises its explicit compatibility fallback instead of treating the
    // LocalStartup catch-all 404 as a malformed refusal.
    await page.route('**/api/v1/**usage/metrics/initial**', (route) =>
      route.fulfill({
        status: 501,
        json: { status: 501, title: 'Disabled', code: 'report_view_disabled' },
      }),
    );
    await page.route('**/api/v1/**usage/metrics/view**', (route) =>
      route.fulfill({
        status: 501,
        json: { status: 501, title: 'Disabled', code: 'report_view_disabled' },
      }),
    );
    await page.route('**/api/v1/**usage/metrics/reports**', async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      this.reads.push(url);
      const admin = url.pathname.includes('/admin/');
      if (this.failure)
        return route.fulfill({
          status: this.failure,
          headers:
            this.retryAfterSeconds === undefined
              ? {}
              : { 'Retry-After': String(this.retryAfterSeconds) },
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
        let report: UsageMetricReport = {
          ...base,
          scope: { principal: 'local-owner', admin },
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
            sandbox_count:
              this.summaryCount?.(input) ??
              (input.filters?.search === 'empty' || input.filters?.statuses?.includes('retired')
                ? 0
                : 25001),
            running_count:
              this.runningCount?.(input) ?? (input.filters?.search === 'empty' ? 0 : 18000),
          },
        };
        if (this.presentationRows && report.summary) {
          const selected = input.filters?.search === 'empty' ? [] : this.presentationRows;
          const allocated = { ...report.summary.allocated_period };
          const capacity = { ...report.summary.total_allocated_capacity };
          for (const field of [
            'cpu_core_hours',
            'memory_gib_hours',
            'disk_gib_hours',
            'uptime_seconds',
            'disk_retained_seconds',
            'unknown_seconds',
            'projected_tail_seconds',
          ] as const)
            allocated[field] = selected.reduce(
              (sum, row) => sum + Number(row.allocated_period[field]),
              0,
            );
          for (const field of ['cpu_cores', 'memory_bytes', 'disk_bytes'] as const)
            capacity[field] = selected.reduce((sum, row) => sum + Number(row.allocation[field]), 0);
          report = {
            ...report,
            summary: {
              ...report.summary,
              sandbox_count: selected.length,
              running_count: selected.filter((row) => row.status === 'active').length,
              allocated_period: {
                ...allocated,
                complete: selected.every((row) => row.allocated_period.complete),
              },
              total_allocated_capacity: capacity,
            },
          };
        }
        if (this.allocatedPeriod && report.summary)
          report = {
            ...report,
            summary: { ...report.summary, allocated_period: this.allocatedPeriod(input) },
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
      if (member === 'owner-allocations') {
        if (this.ownerFailure)
          return route.fulfill({
            status: this.ownerFailure,
            json: { status: this.ownerFailure, title: 'Unavailable', code: 'usage_unavailable' },
          });
        const metric = url.searchParams.get('metric')!;
        const offset = Number(url.searchParams.get('cursor') ?? 0);
        const limit = Number(url.searchParams.get('limit') ?? 50);
        const count = report.query.filters.search === 'empty' ? 0 : this.ownerCount;
        return route.fulfill({
          json: {
            report_id: id,
            metric,
            unit: metric === 'cpu' ? 'core-hours' : 'GiB-hours',
            items: Array.from(
              { length: Math.min(limit, Math.max(0, count - offset)) },
              (_, index) => {
                const n = offset + index;
                return {
                  subject: `91892afa-5fa5-4f4f-9100-${String(n).padStart(12, '0')}`,
                  label:
                    n < 2
                      ? 'Alex Chen'
                      : n === 2
                        ? 'Unnamed owner'
                        : n === 3
                          ? 'Alexandra Montgomery Research Operations'
                          : `Owner ${String(n).padStart(5, '0')}`,
                  ...(n < 2 ? { disambiguator: `Joined 2026-0${n + 1}-01` } : {}),
                  allocated_hours: (count - n) * (metric === 'cpu' ? 1 : 2),
                  sandbox_count: 2,
                  complete: n !== 0,
                  ...this.ownerOverrides.get(n),
                };
              },
            ),
            total_count: count,
            next: offset + limit < count ? String(offset + limit) : null,
          },
        });
      }
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
        const owned = (row: UsageMetricRow) =>
          admin ? row : { ...row, owner: { ...row.owner, subject: report.scope.principal } };
        if (resource)
          return route.fulfill({
            json: owned(
              this.presentationRows?.find((row) => row.sandbox_id === resource) ?? {
                ...metricRow,
                sandbox_id: resource,
              },
            ),
          });
        const second = url.searchParams.has('cursor');
        return route.fulfill({
          json: {
            report_id: id,
            as_of: asOf,
            items:
              report.query.filters.search === 'empty' ||
              report.query.filters.statuses.includes('retired')
                ? []
                : (
                    this.presentationRows ?? [
                      {
                        ...metricRow,
                        ...(second ? { sandbox_id: 'sbx-2', name: 'Second page Sandbox' } : {}),
                      },
                    ]
                  ).map(owned),
            next: this.presentationRows || second ? null : 'page-two',
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
              report.query.filters.search === 'empty' ||
              report.query.filters.statuses.includes('retired')
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
