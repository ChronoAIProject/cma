import { vi } from 'vitest';
import type { UsageMetricReport, UsageMetricRow, UsageMetricSeries } from '@chronoai/cma-sdk';

export const asOf = Math.floor(Date.now() / 1000);
const common = {
  state: 'partial' as const,
  observed_at: asOf - 60,
  source: 'Fixture runtime measurements',
  covered_seconds: 21600,
  expected_seconds: 86400,
  unknown_mapping_seconds: 120,
  coverage_ratio: 0.25,
  available_from: asOf - 86400,
  last_observed: 1,
  percent_of_limit: null,
  filesystem_capacity_bytes: null,
  exceeds_capacity: false,
};
export const metricRow: UsageMetricRow = {
  sandbox_id: 'sbx-1',
  name: 'Research Sandbox',
  owner: { subject: 'owner-1', display_name: 'Alex Chen' },
  status: 'active',
  runtime_version: 'runtime-2',
  agent_count: 2,
  last_activity_at: asOf - 10,
  allocation: { cpu_cores: 2, memory_bytes: 4 * 2 ** 30, disk_bytes: 20 * 2 ** 30 },
  allocated_period: {
    disk_retained_seconds: 86400,
    uptime_seconds: 43200,
    cpu_core_hours: 24,
    memory_gib_hours: 48,
    disk_gib_hours: 480,
    complete: false,
    projected_tail_seconds: 30,
    unknown_seconds: 3600,
  },
  measured: {
    cpu: {
      ...common,
      latest_cores: 0.75,
      mean_cores: 0.5,
      max_cores: 1.25,
      observed_core_hours: 3,
    },
    memory: {
      ...common,
      latest_bytes: 2 ** 30,
      mean_bytes: 2 ** 29,
      max_bytes: 2 ** 30,
      observed_gib_hours: 3,
    },
    disk: {
      ...common,
      state: 'unmounted',
      observed_at: asOf - 7200,
      latest_bytes: null,
      mean_bytes: 4 * 2 ** 30,
      max_bytes: 4 * 2 ** 30,
      last_observed: 4 * 2 ** 30,
      observed_gib_hours: 24,
      filesystem_capacity_bytes: 18 * 2 ** 30,
    },
  },
  billing: { basis: 'allocated', status: 'not_integrated', charges: null },
  shared_sandbox_usage: true,
};
export function reportFixture(admin = false, reportId = 'report-one'): UsageMetricReport {
  return {
    schema_version: 1,
    report_id: reportId,
    status: 'ready',
    report_complete: true,
    scope: { principal: 'owner-1', admin },
    query: {
      from: asOf - 86400,
      to: asOf,
      step_seconds: 60,
      filters: {
        owner: null,
        statuses: [],
        runtime_version: null,
        sandbox_id: null,
        agent_id: null,
        search: '',
      },
    },
    as_of: asOf,
    measurement_cutoff: asOf - 120,
    effective_range: { from: asOf - 86400, to: asOf - 120 },
    expires_at: asOf + 900,
    allocation_watermark: 'fixture-watermark',
    mapping_revision: 1,
    available_from: asOf - 86400,
    tracking_started_at: asOf - 90000,
    warnings: ['Partial source coverage'],
    source_health: 'partial',
    poll_url: '',
    retry_after_seconds: 1,
    summary: {
      sandbox_count: 25001,
      running_count: 18000,
      allocated_period: metricRow.allocated_period,
      total_allocated_capacity: metricRow.allocation,
      covered_allocated_capacity: metricRow.allocation,
      measured: metricRow.measured,
      excluded_count: 1,
      infrastructure: admin
        ? {
            state: 'partial',
            unproven_count: 2,
            unassigned_pool: [2, 2 ** 30, 20 * 2 ** 30],
            unattributed: [null, null, null],
            observed_at: asOf - 60,
          }
        : null,
    },
  };
}
export function seriesFixture(
  metric: 'cpu' | 'memory' | 'disk',
  reportId = 'report-one',
): UsageMetricSeries {
  const observed = {
    from: asOf - 120,
    to: asOf - 60,
    mean: metric === 'cpu' ? 0.5 : 2 ** 30,
    min: metric === 'cpu' ? 0.25 : 2 ** 29,
    max: metric === 'cpu' ? 0.75 : 2 ** 30,
    covered_seconds: 30,
    expected_seconds: 60,
    unknown_mapping_seconds: 0,
    numerator: 30,
    denominator: 120,
    reason: 'partial_coverage',
    excluded_count: 1,
  };
  return {
    report_id: reportId,
    metric,
    unit: metric === 'cpu' ? 'cores' : 'bytes',
    source: common.source,
    lines: [
      {
        id: 'total',
        label: 'Total',
        points: [
          observed,
          {
            ...observed,
            from: asOf - 60,
            to: asOf,
            mean: null,
            min: null,
            max: null,
            covered_seconds: 0,
            numerator: null,
            denominator: null,
            reason: 'no_measurement',
          },
        ],
      },
    ],
  };
}
export function metricsClient(admin = false) {
  return {
    create: vi.fn().mockResolvedValue(reportFixture(admin)),
    report: vi.fn().mockResolvedValue(reportFixture(admin)),
    sandboxes: vi.fn().mockResolvedValue({
      report_id: 'report-one',
      as_of: asOf,
      items: [metricRow],
      next: 'page-two',
      total_count: 25001,
    }),
    sandbox: vi.fn().mockResolvedValue(metricRow),
    owners: vi.fn().mockResolvedValue({
      report_id: 'report-one',
      items: [{ subject: 'owner-1', display_name: 'Alex Chen' }],
      next: 'owners-two',
    }),
    series: vi
      .fn()
      .mockImplementation((id: string, options: { metric: 'cpu' | 'memory' | 'disk' }) =>
        Promise.resolve(seriesFixture(options.metric, id)),
      ),
  };
}
