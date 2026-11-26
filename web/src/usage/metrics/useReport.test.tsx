import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { defaultSelection } from './model';
import { asOf, metricsClient, reportFixture } from './testFixtures';
import { prefetchUsageReport, useUsageReport } from './useReport';

afterEach(() => vi.useRealTimers());
it('polls a pending report and resumes that report after the popup is hidden', async () => {
  vi.useFakeTimers();
  const api = metricsClient();
  api.create.mockResolvedValueOnce({ ...reportFixture(), status: 'pending', summary: null });
  const { result, rerender } = renderHook(
    ({ active }) => useUsageReport(api, defaultSelection, false, active),
    { initialProps: { active: true } },
  );
  await act(async () => {});
  expect(result.current.pending?.report_id).toBe('report-one');
  rerender({ active: false });
  await act(async () => vi.advanceTimersByTimeAsync(2000));
  expect(api.report).not.toHaveBeenCalled();
  rerender({ active: true });
  await act(async () => vi.advanceTimersByTimeAsync(2000));
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(api.report).toHaveBeenCalledWith(
    'report-one',
    expect.objectContaining({ signal: expect.any(AbortSignal) }),
  );
  expect(result.current.report?.status).toBe('ready');
});
it('pauses automatic refresh while hidden and coalesces rapid visibility changes', async () => {
  vi.useFakeTimers();
  const api = metricsClient();
  const { rerender } = renderHook(
    ({ active }) => useUsageReport(api, defaultSelection, false, active),
    { initialProps: { active: true } },
  );
  await act(async () => {});
  rerender({ active: false });
  rerender({ active: true });
  await act(async () => {});
  expect(api.create).toHaveBeenCalledTimes(1);
  rerender({ active: false });
  await act(async () => vi.advanceTimersByTimeAsync(180000));
  expect(api.create).toHaveBeenCalledTimes(1);
  rerender({ active: true });
  await act(async () => {});
  expect(api.create).toHaveBeenCalledTimes(2);
  await act(async () => vi.advanceTimersByTimeAsync(59999));
  expect(api.create).toHaveBeenCalledTimes(2);
  await act(async () => vi.advanceTimersByTimeAsync(1));
  expect(api.create).toHaveBeenCalledTimes(3);
});
it('does not retain a different selection during a transient retry or terminal expiry', async () => {
  vi.useFakeTimers();
  const api = metricsClient();
  const { result, rerender } = renderHook(
    ({ selection }) => useUsageReport(api, selection, false),
    { initialProps: { selection: defaultSelection } },
  );
  await act(async () => {});
  api.create.mockRejectedValueOnce(new ApiError(503, 'Unavailable'));
  api.create.mockRejectedValueOnce(new ApiError(410, 'Expired'));
  rerender({ selection: { ...defaultSelection, sandbox: 'sbx-2' } });
  await act(async () => {});
  expect(result.current.retrying).toBe(true);
  expect(result.current.stale).toBe(false);
  expect(result.current.report).toBeUndefined();
  await act(async () => vi.advanceTimersByTimeAsync(1200));
  expect(result.current.error?.expired).toBe(true);
  expect(result.current.report).toBeUndefined();
});
it('distinguishes a failed report from an empty successful report', async () => {
  const api = metricsClient();
  api.create.mockResolvedValueOnce({ ...reportFixture(), status: 'failed', summary: null });
  const { result } = renderHook(() => useUsageReport(api, defaultSelection, false));
  await waitFor(() => expect(result.current.error?.title).toBe('Could not load usage'));
  expect(result.current.report).toBeUndefined();
});
it('fetches only while active and isolates each selection cache entry', async () => {
  const api = metricsClient();
  const { result, rerender, unmount } = renderHook(
    ({ selection, active }) => useUsageReport(api, selection, false, active),
    { initialProps: { selection: defaultSelection, active: false } },
  );
  expect(api.create).not.toHaveBeenCalled();
  rerender({ selection: defaultSelection, active: true });
  await waitFor(() => expect(result.current.report?.report_id).toBe('report-one'));
  const signal = api.create.mock.calls[0]![1].signal as AbortSignal;
  rerender({ selection: { ...defaultSelection, sandbox: 'sbx-2' }, active: true });
  await waitFor(() => expect(api.create).toHaveBeenCalledTimes(2));
  expect(signal.aborted).toBe(false);
  expect(api.create.mock.calls[1]![0].filters.sandbox_id).toBe('sbx-2');
  unmount();
  expect(api.create.mock.calls[1]![1].signal.aborted).toBe(false);
});
it('sorts using the same immutable report rather than creating another snapshot', async () => {
  const api = metricsClient(true);
  const { result, rerender } = renderHook(({ selection }) => useUsageReport(api, selection, true), {
    initialProps: { selection: defaultSelection },
  });
  await waitFor(() => expect(result.current.report).toBeDefined());
  rerender({ selection: { ...defaultSelection, sort: 'cpu_mean_cores', order: 'asc' } });
  expect(api.create).toHaveBeenCalledTimes(1);
});
it('ignores a late response from the previous selection', async () => {
  const api = metricsClient();
  let resolveOld!: (value: ReturnType<typeof reportFixture>) => void;
  api.create.mockReturnValueOnce(
    new Promise((resolve) => {
      resolveOld = resolve;
    }),
  );
  api.create.mockResolvedValueOnce(reportFixture(false, 'new-report'));
  const { result, rerender } = renderHook(
    ({ selection }) => useUsageReport(api, selection, false),
    { initialProps: { selection: defaultSelection } },
  );
  await act(async () => {});
  rerender({ selection: { ...defaultSelection, sandbox: 'sbx-2' } });
  await waitFor(() => expect(result.current.report?.report_id).toBe('new-report'));
  await act(async () => resolveOld(reportFixture(false, 'old-report')));
  expect(result.current.report?.report_id).toBe('new-report');
});
it.each([401, 403, 404])('clears retained report data after access refusal %s', async (status) => {
  const api = metricsClient();
  const { result } = renderHook(() => useUsageReport(api, defaultSelection, false));
  await waitFor(() => expect(result.current.report).toBeDefined());
  api.create.mockRejectedValueOnce(new ApiError(status, 'Refused'));
  act(() => result.current.refresh());
  await waitFor(() => expect(result.current.error?.denied).toBe(true));
  expect(result.current.report).toBeUndefined();
});
it('honors backoff before automatically retrying without showing a zero total', async () => {
  const api = metricsClient();
  api.create.mockRejectedValueOnce(new ApiError(429, 'Busy', undefined, undefined, 60000));
  const { result } = renderHook(() => useUsageReport(api, defaultSelection, false));
  await waitFor(() => expect(result.current.retrying).toBe(true));
  expect(result.current.nextCheckAt).toBeGreaterThan(Date.now());
  act(() => result.current.refresh());
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(result.current.report).toBeUndefined();
});

it('reuses an exact-key committed report immediately after remounting', async () => {
  const api = metricsClient();
  const first = renderHook(() => useUsageReport(api, defaultSelection, false));
  await waitFor(() => expect(first.result.current.report?.report_id).toBe('report-one'));
  first.unmount();
  api.create.mockResolvedValueOnce({ ...reportFixture(false), report_id: 'report-two' });
  const second = renderHook(() => useUsageReport(api, defaultSelection, false));
  expect(second.result.current.report?.report_id).toBe('report-one');
  await act(async () => {});
  expect(second.result.current.report?.report_id).toBe('report-one');
  expect(api.create).toHaveBeenCalledTimes(1);
});

it('coalesces prefetch with the mounted report request', async () => {
  const api = metricsClient(true);
  prefetchUsageReport(api, defaultSelection, true);
  await waitFor(() => expect(api.create).toHaveBeenCalledTimes(1));
  const { result } = renderHook(() => useUsageReport(api, defaultSelection, true));
  await waitFor(() => expect(result.current.report?.report_id).toBe('report-one'));
  expect(api.create).toHaveBeenCalledTimes(1);
});

it('does not publish a report across principals on a reused SDK client', async () => {
  const api = metricsClient();
  api.create.mockResolvedValueOnce({
    ...reportFixture(),
    scope: { principal: 'owner-a', admin: false },
  });
  const { result, rerender } = renderHook(
    ({ owner }) => useUsageReport(api, defaultSelection, false, true, owner),
    { initialProps: { owner: { principal: 'owner-a', cacheKey: 'owner-a:capability-a' } } },
  );
  await waitFor(() => expect(result.current.report?.scope.principal).toBe('owner-a'));
  api.create.mockRejectedValueOnce(new ApiError(503, 'Unavailable'));
  rerender({ owner: { principal: 'owner-b', cacheKey: 'owner-b:capability-b' } });
  expect(result.current.report).toBeUndefined();
  await waitFor(() => expect(result.current.retrying).toBe(true));
  expect(result.current.report).toBeUndefined();
});

it('keeps a punctuation-containing principal separate from its cache identity', async () => {
  const api = metricsClient();
  const latest = vi.fn().mockResolvedValue({
    report: { ...reportFixture(), scope: { principal: 'realm:alice', admin: false } },
    generated_at: asOf,
    stale: false,
    refresh: { state: 'idle' as const, report_id: null, retry_after_seconds: 0 },
  });
  (api as unknown as { latest: typeof latest }).latest = latest;
  const owner = { principal: 'realm:alice', cacheKey: 'realm:alice|caps|epoch-1' };
  const { result } = renderHook(() => useUsageReport(api, defaultSelection, false, true, owner));
  await waitFor(() => expect(result.current.report?.scope.principal).toBe('realm:alice'));
  expect(result.current.error).toBeUndefined();
});

it('clears all mounted entries after a retained report authority mismatch', async () => {
  const api = metricsClient();
  const owner = { principal: 'owner-a', cacheKey: 'owner-a|caps|epoch-1' };
  const authorized = () => ({
    ...reportFixture(false),
    scope: { principal: 'owner-a', admin: false },
  });
  api.create.mockImplementation(async (query) => ({
    ...authorized(),
    report_id: query.filters.sandbox_id ? 'sandbox-report' : 'default-report',
  }));
  const first = renderHook(() => useUsageReport(api, defaultSelection, false, true, owner));
  const second = renderHook(() =>
    useUsageReport(api, { ...defaultSelection, sandbox: 'sbx-2' }, false, true, owner),
  );
  await waitFor(() => expect(first.result.current.report).toBeDefined());
  await waitFor(() => expect(second.result.current.report).toBeDefined());
  api.create.mockResolvedValueOnce({
    ...authorized(),
    scope: { principal: 'owner-b', admin: false },
  });
  act(() => first.result.current.refresh());
  await waitFor(() => expect(first.result.current.error?.denied).toBe(true));
  expect(first.result.current.report).toBeUndefined();
  await waitFor(() => expect(second.result.current.report).toBeUndefined());
});

it('renders a committed latest view while its refresh continues', async () => {
  vi.useFakeTimers();
  const api = metricsClient(true);
  api.report.mockResolvedValue({ ...reportFixture(true), report_id: 'refresh-one' });
  const latest = vi.fn(function (this: unknown, input: Record<string, unknown>) {
    expect(this).toBe(api);
    expect(input).not.toHaveProperty('from');
    expect(input).not.toHaveProperty('to');
    return Promise.resolve(
      latest.mock.calls.length === 1
        ? {
            report: reportFixture(true),
            generated_at: asOf,
            stale: true,
            refresh: {
              state: 'pending' as const,
              report_id: 'refresh-one',
              retry_after_seconds: 1,
            },
          }
        : {
            report: { ...reportFixture(true), report_id: 'report-two' },
            generated_at: asOf + 10,
            stale: false,
            refresh: { state: 'idle' as const, report_id: null, retry_after_seconds: 0 },
          },
    );
  });
  (api as unknown as { latest: typeof latest }).latest = latest;
  const { result, rerender } = renderHook(
    ({ active }) => useUsageReport(api, defaultSelection, true, active),
    { initialProps: { active: true } },
  );
  await act(async () => {});
  expect(result.current.report?.report_id).toBe('report-one');
  expect(result.current.busy).toBe(false);
  expect(result.current.refreshing).toBe(true);
  rerender({ active: false });
  await act(async () => vi.advanceTimersByTimeAsync(2000));
  expect(latest).toHaveBeenCalledTimes(1);
  rerender({ active: true });
  await act(async () => vi.advanceTimersByTimeAsync(2000));
  expect(result.current.report?.report_id).toBe('report-two');
  expect(result.current.generatedAt).toBe(asOf + 10);
  expect(result.current.refreshing).toBe(false);
});

it('uses the legacy report flow only for an explicitly unsupported latest route', async () => {
  const api = metricsClient(true);
  const latest = vi.fn().mockRejectedValue(new ApiError(501, 'disabled', 'report_view_disabled'));
  (api as unknown as { latest: typeof latest }).latest = latest;
  const { result } = renderHook(() => useUsageReport(api, defaultSelection, true));
  await waitFor(() => expect(result.current.report?.report_id).toBe('report-one'));
  expect(latest).toHaveBeenCalledTimes(1);
  expect(api.create).toHaveBeenCalledTimes(1);
});

it('reuses a disabled view negotiation for repeated selections within the same session', async () => {
  const api = metricsClient(true);
  const latest = vi.fn().mockRejectedValue(new ApiError(501, 'disabled', 'report_view_disabled'));
  (api as unknown as { latest: typeof latest }).latest = latest;
  const first = renderHook(() => useUsageReport(api, defaultSelection, true));
  await waitFor(() => expect(first.result.current.report?.report_id).toBe('report-one'));
  first.unmount();

  const nextSelection = { ...defaultSelection, range: '7d' as const };
  const second = renderHook(() => useUsageReport(api, nextSelection, true));
  await waitFor(() => expect(second.result.current.report?.report_id).toBe('report-one'));
  expect(latest).toHaveBeenCalledTimes(1);
  expect(api.create).toHaveBeenCalledTimes(2);
});
