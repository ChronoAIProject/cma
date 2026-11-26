import { act, renderHook, waitFor } from '@testing-library/react';
import { ApiError } from '@cma/client';
import { afterEach, expect, it, vi } from 'vitest';
import { defaultSelection, type MetricsClient } from './model';
import { metricRow, metricsClient, reportFixture } from './testFixtures';
import { useUsageReport } from './useReport';

function initialPage(reportId: string) {
  return {
    report_id: reportId,
    as_of: reportFixture(true).as_of,
    items: [metricRow],
    next: 'page-two',
    total_count: 25001,
  };
}

it('uses the bounded initial payload for the first page and leaves child reads idle', async () => {
  const api = metricsClient(true) as ReturnType<typeof metricsClient> & {
    createInitial: ReturnType<typeof vi.fn>;
    latestInitial: ReturnType<typeof vi.fn>;
    initialReport: ReturnType<typeof vi.fn>;
  };
  const report = reportFixture(true);
  api.createInitial = vi.fn();
  api.latestInitial = vi.fn().mockResolvedValue({
    view: {
      report,
      schema_version: 1,
      window: '24h',
      requested_range: { from: report.query.from, to: report.query.to },
      generated_at: report.as_of,
      stale: false,
      refresh: { state: 'idle', report_id: null, retry_after_seconds: 0 },
    },
    initial: {
      projection: {
        first_page: { limit: 50, sort: 'last_activity_at', order: 'desc' },
        total_metrics: [],
      },
      rows: initialPage(report.report_id),
      charts: [],
    },
  });
  api.initialReport = vi.fn();

  const { result } = renderHook(() =>
    useUsageReport(
      api as unknown as MetricsClient,
      defaultSelection,
      true,
      true,
      undefined,
      false,
      false,
      true,
    ),
  );
  await waitFor(() => expect(result.current.report?.report_id).toBe(report.report_id));

  expect(result.current.initial?.rows.items).toHaveLength(1);
  expect(result.current.initialProjection?.first_page?.sort).toBe('last_activity_at');
  expect(api.latestInitial).toHaveBeenCalledTimes(1);
  expect(api.initialReport).not.toHaveBeenCalled();
  expect(api.sandboxes).not.toHaveBeenCalled();
  expect(api.series).not.toHaveBeenCalled();
});

it('falls back to the existing latest and report flow only when the initial route is absent', async () => {
  const api = metricsClient(true) as ReturnType<typeof metricsClient> & {
    createInitial: ReturnType<typeof vi.fn>;
    initialReport: ReturnType<typeof vi.fn>;
    latest: ReturnType<typeof vi.fn>;
  };
  api.createInitial = vi
    .fn()
    .mockRejectedValue(new ApiError(501, 'disabled', 'report_view_disabled'));
  api.initialReport = vi.fn();
  api.latest = vi.fn().mockResolvedValue({
    report: reportFixture(true),
    generated_at: 1,
    stale: false,
    refresh: { state: 'idle', report_id: null, retry_after_seconds: 0 },
  });

  const { result } = renderHook(() =>
    useUsageReport(
      api as unknown as MetricsClient,
      defaultSelection,
      true,
      true,
      undefined,
      false,
      false,
      true,
    ),
  );
  await waitFor(() => expect(result.current.report?.report_id).toBe('report-one'));

  expect(api.createInitial).toHaveBeenCalledTimes(1);
  expect(api.latest).toHaveBeenCalledTimes(1);
  expect(api.create).not.toHaveBeenCalled();
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it.each(['pending', 'deferred'] as const)(
  'retains an initial Ready view during a %s refresh',
  async (state) => {
    vi.useFakeTimers();
    vi.spyOn(Math, 'random').mockReturnValue(0);
    const report = reportFixture(true);
    const replacement = { ...report, report_id: 'successor' };
    const initial = (id: string) => ({
      projection: {
        first_page: { limit: 50, sort: 'last_activity_at', order: 'desc' },
        total_metrics: [],
      },
      rows: initialPage(id),
      charts: [],
    });
    const view = {
      report,
      generated_at: report.as_of,
      stale: true,
      refresh: {
        state,
        report_id: state === 'pending' ? replacement.report_id : null,
        retry_after_seconds: 2,
      },
    };
    const api = Object.assign(metricsClient(true), {
      createInitial: vi.fn(),
      latestInitial: vi
        .fn()
        .mockResolvedValueOnce({ view, initial: initial(report.report_id) })
        .mockResolvedValue({
          view: {
            ...view,
            report: replacement,
            stale: false,
            refresh: { state: 'idle', report_id: null, retry_after_seconds: 0 },
          },
          initial: initial(replacement.report_id),
        }),
      initialReport: vi
        .fn()
        .mockResolvedValue({ report: replacement, initial: initial(replacement.report_id) }),
    });
    const hook = renderHook(() =>
      useUsageReport(
        api as unknown as MetricsClient,
        defaultSelection,
        true,
        true,
        undefined,
        false,
        false,
        true,
      ),
    );
    await act(async () => {});
    expect(hook.result.current.report?.report_id).toBe(report.report_id);
    expect(hook.result.current.initial?.rows.report_id).toBe(report.report_id);
    expect(hook.result.current.stale).toBe(true);
    expect(hook.result.current.generatedAt).toBe(Number(report.as_of));
    if (state === 'deferred') act(() => hook.result.current.refresh());
    await act(async () => vi.advanceTimersByTimeAsync(1999));
    expect(api.initialReport).not.toHaveBeenCalled();
    expect(api.latestInitial).toHaveBeenCalledTimes(1);
    await act(async () => vi.advanceTimersByTimeAsync(1));
    if (state === 'pending') {
      expect(api.initialReport).toHaveBeenCalledTimes(1);
      expect(api.initialReport.mock.calls[0]?.[0]).toBe(replacement.report_id);
      expect(api.latestInitial).toHaveBeenCalledTimes(1);
    } else {
      expect(api.initialReport).not.toHaveBeenCalled();
      expect(api.latestInitial).toHaveBeenCalledTimes(2);
    }
    expect(hook.result.current.report?.report_id).toBe(replacement.report_id);
    expect(hook.result.current.stale).toBe(false);
  },
);
