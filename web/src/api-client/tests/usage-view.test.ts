// @vitest-environment node
import { expect, it } from 'vitest';
import { ApiError, ProtocolError, isUsageViewUnavailable } from '../index.js';
import { transport } from './resource-fixtures.js';
function pending() {
  const reportId = 'umr_00000000000000000000000000000001';
  const range = { from: 1_700_000_000, to: 1_700_086_400 };
  return {
    schema_version: 1,
    window: '24h',
    requested_range: range,
    generated_at: null,
    stale: false,
    refresh: { state: 'pending', report_id: reportId, retry_after_seconds: 2 },
    report: {
      schema_version: 1,
      report_id: reportId,
      status: 'pending',
      report_complete: false,
      scope: { principal: 'owner', admin: true },
      query: {
        ...range,
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
      as_of: range.to,
      measurement_cutoff: range.to - 120,
      effective_range: range,
      expires_at: range.to + 86400,
      allocation_watermark: '',
      mapping_revision: 0,
      available_from: null,
      tracking_started_at: null,
      warnings: [],
      source_health: 'pending',
      summary: null,
      poll_url: `/api/v1/admin/usage/metrics/reports/${reportId}`,
      retry_after_seconds: 2,
    },
  };
}
it('uses the dedicated route and returns pending identity without polling', async () => {
  const test = transport();
  const value = pending();
  test.answer(value, 202);
  expect(await test.client.adminUsageMetrics.latest()).toEqual(value);
  expect(test.calls).toHaveLength(1);
  expect(test.calls[0]?.url).toBe(
    'https://cma.example.test/api/v1/admin/usage/metrics/view?window=24h',
  );
});
it('rejects foreign selections and mixed pending/committed envelopes', async () => {
  for (const mutate of [
    (v: ReturnType<typeof pending>) => {
      v.report.scope.admin = false;
    },
    (v: ReturnType<typeof pending>) => {
      v.report.query.filters.search = 'foreign';
    },
    (v: ReturnType<typeof pending>) => {
      v.refresh.report_id = 'another';
    },
    (v: ReturnType<typeof pending>) => {
      v.stale = true;
    },
    (v: ReturnType<typeof pending>) => {
      v.report.status = 'ready';
    },
  ]) {
    const test = transport();
    const value = pending();
    mutate(value);
    test.answer(value, 200);
    await expect(test.client.adminUsageMetrics.latest()).rejects.toBeInstanceOf(ProtocolError);
    expect(test.calls).toHaveLength(1);
  }
});
it('allows compatibility fallback only for explicit feature or route absence', () => {
  expect(isUsageViewUnavailable(new ApiError(501, 'off', 'report_view_disabled'))).toBe(true);
  expect(isUsageViewUnavailable(new ApiError(404, 'route missing'))).toBe(true);
  for (const error of [
    new ApiError(404, 'resource', 'not_found'),
    new ApiError(404, 'resource', undefined, undefined, undefined, { reason: 'concealed' }),
    new ApiError(403, 'denied'),
    new ApiError(503, 'busy'),
    new ApiError(501, 'other', 'different'),
    new ProtocolError('invalid'),
  ])
    expect(isUsageViewUnavailable(error)).toBe(false);
});
it('returns stale committed summary with independent replacement state', async () => {
  const test = transport();
  const base = pending();
  const capacity = { cpu_cores: 0, memory_bytes: 0, disk_bytes: 0 };
  const value = {
    ...base,
    generated_at: base.report.as_of,
    stale: true,
    refresh: {
      state: 'pending',
      report_id: 'umr_00000000000000000000000000000002',
      retry_after_seconds: 2,
    },
    report: {
      ...base.report,
      status: 'ready',
      report_complete: true,
      summary: {
        sandbox_count: 0,
        running_count: 0,
        allocated_period: {
          uptime_seconds: 0,
          disk_retained_seconds: 0,
          cpu_core_hours: 0,
          memory_gib_hours: 0,
          disk_gib_hours: 0,
          complete: true,
          projected_tail_seconds: 0,
          unknown_seconds: 0,
        },
        total_allocated_capacity: capacity,
        covered_allocated_capacity: capacity,
        measured: null,
        excluded_count: 0,
        infrastructure: null,
      },
    },
  };
  test.answer(value);
  expect(await test.client.adminUsageMetrics.latest()).toEqual(value);
  expect(test.calls).toHaveLength(1);
});
