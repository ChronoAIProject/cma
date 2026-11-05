import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { defaultSelection } from './model';
import { metricsClient, reportFixture } from './testFixtures';
import { useUsageReport } from './useReport';

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
  await act(async () => vi.advanceTimersByTimeAsync(1000));
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
it('labels the retained selection when a replacement fails, then clears expired data', async () => {
  const api = metricsClient();
  const { result, rerender } = renderHook(
    ({ selection }) => useUsageReport(api, selection, false),
    { initialProps: { selection: defaultSelection } },
  );
  await waitFor(() => expect(result.current.report).toBeDefined());
  api.create.mockRejectedValueOnce(new ApiError(503, 'Unavailable'));
  rerender({ selection: { ...defaultSelection, sandbox: 'sbx-2' } });
  await waitFor(() => expect(result.current.error?.title).toBe('Usage reporting is unavailable'));
  expect(result.current.stale).toBe(true);
  expect(result.current.report?.report_id).toBe('report-one');
  api.create.mockRejectedValueOnce(new ApiError(410, 'Expired'));
  act(() => result.current.refresh());
  await waitFor(() => expect(result.current.error?.expired).toBe(true));
  expect(result.current.report).toBeUndefined();
});
it('distinguishes a failed report from an empty successful report', async () => {
  const api = metricsClient();
  api.create.mockResolvedValueOnce({ ...reportFixture(), status: 'failed', summary: null });
  const { result } = renderHook(() => useUsageReport(api, defaultSelection, false));
  await waitFor(() => expect(result.current.error?.title).toBe('Could not load usage'));
  expect(result.current.report).toBeUndefined();
});
it('fetches only while active and aborts a superseded selection', async () => {
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
  expect(signal.aborted).toBe(true);
  expect(api.create.mock.calls[1]![0].filters.sandbox_id).toBe('sbx-2');
  unmount();
  expect(api.create.mock.calls[1]![1].signal.aborted).toBe(true);
});
it('sorts using the same immutable report rather than creating another snapshot', async () => {
  const api = metricsClient();
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
it('honors backoff without automatically retrying or showing a zero total', async () => {
  const api = metricsClient();
  api.create.mockRejectedValueOnce(new ApiError(429, 'Busy', undefined, undefined, 60000));
  const { result } = renderHook(() => useUsageReport(api, defaultSelection, false));
  await waitFor(() => expect(result.current.error?.retryAt).toBeDefined());
  act(() => result.current.refresh());
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(result.current.report).toBeUndefined();
});
