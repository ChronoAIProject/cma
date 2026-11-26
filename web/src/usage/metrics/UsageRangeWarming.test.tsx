import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { defaultSelection, type Selection } from './model';
import { metricsClient, reportFixture } from './testFixtures';
import { useUsageReport } from './useReport';
import { useWarmUsageRanges } from './warmRanges';
import { reportRequest } from './reportRequests';
import type { LatestUsageMetricView } from './latest';

const owner = { principal: 'owner-1', cacheKey: 'warming-epoch' };
const flush = () => act(async () => {});
const ready = (id: string, admin: boolean): LatestUsageMetricView => ({
  report: reportFixture(admin, id),
  generated_at: 1,
  stale: false,
  refresh: { state: 'idle', report_id: null, retry_after_seconds: 0 },
});
const pending = (id: string, admin: boolean): LatestUsageMetricView => ({
  ...ready(id, admin),
  generated_at: null,
  report: { ...reportFixture(admin, id), status: 'pending', summary: null },
  refresh: { state: 'pending', report_id: id, retry_after_seconds: 2 },
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it.each([false, true])(
  'admits 7d then 30d once for admin=%s, without background polling',
  async (admin) => {
    vi.useFakeTimers();
    const api = metricsClient(admin);
    const latest = vi.fn(async ({ window }: { window: string }) => pending(window, admin));
    Object.assign(api, { latest });
    const hook = renderHook(
      ({ active }) => useWarmUsageRanges(api, defaultSelection, admin, owner, active),
      { initialProps: { active: true } },
    );
    await flush();
    expect(latest.mock.calls.map(([query]) => query.window)).toEqual(['7d', '30d']);
    await act(async () => vi.advanceTimersByTimeAsync(60000));
    expect(latest).toHaveBeenCalledTimes(2);
    expect(api.report).not.toHaveBeenCalled();
    hook.rerender({ active: false });
    hook.rerender({ active: true });
    await flush();
    expect(latest).toHaveBeenCalledTimes(2);
    hook.unmount();
  },
);

it('adopts a warmed pending ID on selection, preserving its deadline and starting a new visible budget', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  const api = metricsClient();
  const latest = vi.fn(async ({ window }: { window: string }) => pending(window, false));
  Object.assign(api, { latest });
  const warm = renderHook(() => useWarmUsageRanges(api, defaultSelection, false, owner, true));
  await flush();
  warm.unmount();
  await act(async () => vi.advanceTimersByTimeAsync(121000));
  latest.mockImplementation(async ({ window }) => ready(window, false));
  api.report.mockResolvedValue(reportFixture(false, '7d'));
  const selected = { ...defaultSelection, range: '7d' as const };
  const hook = renderHook(() => useUsageReport(api, selected, false, true, owner));
  await flush();
  expect(api.report).toHaveBeenCalledWith('7d', expect.anything());
  expect(latest).toHaveBeenCalledTimes(3);
  expect(hook.result.current.report?.report_id).toBe('7d');
  expect(hook.result.current.error).toBeUndefined();
});

it.each([new ApiError(501, 'Disabled', 'report_view_disabled'), new ApiError(503, 'Busy')])(
  'stops optional warming after a refusal without legacy admission or a retry loop',
  async (error) => {
    vi.useFakeTimers();
    const api = metricsClient();
    const latest = vi.fn().mockRejectedValue(error);
    Object.assign(api, { latest });
    renderHook(() => useWarmUsageRanges(api, defaultSelection, false, owner, true));
    await flush();
    await act(async () => vi.advanceTimersByTimeAsync(180000));
    expect(latest).toHaveBeenCalledTimes(1);
    expect(api.create).not.toHaveBeenCalled();
    expect(api.report).not.toHaveBeenCalled();
  },
);

it('cancels an owned background request on exit and cannot publish its late completion', async () => {
  const api = metricsClient();
  let complete!: (value: ReturnType<typeof ready>) => void;
  const latest = vi.fn(
    (_query, { signal }: { signal: AbortSignal }) =>
      new Promise<ReturnType<typeof ready>>((resolve) => {
        expect(signal.aborted).toBe(false);
        complete = resolve;
      }),
  );
  Object.assign(api, { latest });
  const warm = renderHook(() => useWarmUsageRanges(api, defaultSelection, false, owner, true));
  await flush();
  const signal = latest.mock.calls[0]![1].signal;
  warm.unmount();
  expect(signal.aborted).toBe(true);
  await act(async () => complete(ready('late', false)));
  expect(latest).toHaveBeenCalledTimes(1);
  const selected = { ...defaultSelection, range: '7d' as const };
  const hook = renderHook(() => useUsageReport(api, selected, false, false, owner));
  expect(hook.result.current.report).toBeUndefined();
});

it('lets a selected view adopt an in-flight warmer when its original owner leaves', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  const api = metricsClient();
  let complete!: (value: ReturnType<typeof pending>) => void;
  const latest = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          complete = resolve;
        }),
    )
    .mockResolvedValue(ready('7d', false));
  Object.assign(api, { latest });
  const warm = renderHook(() => useWarmUsageRanges(api, defaultSelection, false, owner, true));
  await flush();
  const selected: Selection = { ...defaultSelection, range: '7d' };
  const hook = renderHook(() => useUsageReport(api, selected, false, true, owner));
  warm.unmount();
  await act(async () => complete(pending('7d', false)));
  expect(latest).toHaveBeenCalledTimes(1);
  api.report.mockResolvedValue(reportFixture(false, '7d'));
  await act(async () => vi.advanceTimersByTimeAsync(2000));
  expect(api.report).toHaveBeenCalledTimes(1);
  expect(hook.result.current.report?.report_id).toBe('7d');
  expect(latest).toHaveBeenCalledTimes(2);
});

it('serializes report calls, prioritizes selected work, and ignores a cancelled late settlement', async () => {
  const client = {};
  const active = new AbortController();
  const second = new AbortController();
  const calls: string[] = [];
  let late!: () => void;
  let finishSelected!: () => void;
  const first = reportRequest(client, active.signal, () => {
    calls.push('first');
    return new Promise<void>((resolve) => {
      late = resolve;
    });
  });
  const refused = first.catch(() => {});
  const background = reportRequest(
    client,
    second.signal,
    async () => {
      calls.push('background');
    },
    true,
  );
  const selected = reportRequest(client, second.signal, () => {
    calls.push('selected');
    return new Promise<void>((resolve) => {
      finishSelected = resolve;
    });
  });
  await flush();
  expect(calls).toEqual(['first']);
  active.abort();
  await refused;
  await flush();
  expect(calls).toEqual(['first', 'selected']);
  late();
  await flush();
  expect(calls).toEqual(['first', 'selected']);
  finishSelected();
  await selected;
  await background;
  expect(calls).toEqual(['first', 'selected', 'background']);
});

it.each(['7d', '30d'] as const)(
  'reads the warmed daily snapshot first on direct %s entry',
  async (range) => {
    const api = metricsClient();
    const latest = vi.fn(async ({ window }: { window: string }) => ready(window, false));
    Object.assign(api, { latest });
    const selected = { ...defaultSelection, range };
    renderHook(() => useWarmUsageRanges(api, selected, false, owner, true));
    await flush();
    expect(latest.mock.calls.map(([query]) => query.window)).toEqual(
      range === '7d' ? ['24h', '30d'] : ['24h', '7d'],
    );
    expect(api.report).not.toHaveBeenCalled();
  },
);

it('does not create or poll unselected ranges after a foreground compatibility fallback', async () => {
  const api = metricsClient();
  const latest = vi.fn().mockRejectedValue(new ApiError(501, 'Disabled', 'report_view_disabled'));
  Object.assign(api, { latest });
  const selected = renderHook(() => useUsageReport(api, defaultSelection, false, true, owner));
  await flush();
  expect(selected.result.current.report?.report_id).toBe('report-one');

  renderHook(() => useWarmUsageRanges(api, defaultSelection, false, owner, true));
  await flush();
  expect(latest).toHaveBeenCalledTimes(1);
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(api.report).not.toHaveBeenCalled();
});

it('keeps an exact ready refresh when the next view replica declines', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  const api = metricsClient();
  const latest = vi
    .fn()
    .mockResolvedValueOnce({
      ...ready('old-snapshot', false),
      refresh: { state: 'pending', report_id: 'completed-refresh', retry_after_seconds: 2 },
    })
    .mockRejectedValueOnce(new ApiError(501, 'Disabled', 'report_view_disabled'));
  Object.assign(api, { latest });
  api.report.mockResolvedValue(reportFixture(false, 'completed-refresh'));
  const selected = renderHook(() => useUsageReport(api, defaultSelection, false, true, owner));
  await flush();
  expect(selected.result.current.report?.report_id).toBe('old-snapshot');
  await act(async () => vi.advanceTimersByTimeAsync(2000));
  expect(api.report).toHaveBeenCalledWith('completed-refresh', expect.anything());
  expect(latest).toHaveBeenCalledTimes(2);
  expect(api.create).not.toHaveBeenCalled();
  expect(selected.result.current.report?.report_id).toBe('completed-refresh');
});
