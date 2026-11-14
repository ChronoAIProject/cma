import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { defaultSelection } from './model';
import { asOf, metricsClient, reportFixture } from './testFixtures';
import { prefetchUsageReport, useUsageReport } from './useReport';
import { liveEntries } from './reportCache';
import { useReportOwner } from './reportOwner';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});
const flush = () => act(async () => {});
const pending = () => ({ ...reportFixture(true), status: 'pending' as const, summary: null });
const view = (state: 'idle' | 'pending' | 'deferred' = 'pending') => ({
  report: reportFixture(true),
  generated_at: asOf,
  stale: state !== 'idle',
  refresh: { state, report_id: state === 'pending' ? 'replacement' : null, retry_after_seconds: 2 },
});

it.each(['capacity', 'ttl', 'descriptor expiry'])(
  'evicts abandoned pending descriptors by %s while releasing live references',
  async (reason) => {
    vi.useFakeTimers();
    const api = metricsClient(true);
    api.create.mockResolvedValue(pending());
    const count = reason === 'capacity' ? 40 : 1;
    const first = { ...defaultSelection, sandbox: '0' };
    for (let index = 0; index < count; index++) {
      const release = prefetchUsageReport(api, { ...first, sandbox: String(index) }, true);
      await flush();
      release();
      await flush();
    }
    expect(liveEntries.size).toBe(0);
    if (reason === 'ttl') await act(async () => vi.advanceTimersByTimeAsync(300001));
    if (reason === 'descriptor expiry') vi.setSystemTime((Number(pending().expires_at) + 1) * 1000);
    const release = prefetchUsageReport(api, first, true);
    await flush();
    expect(api.create).toHaveBeenCalledTimes(count + 1);
    expect(api.report).not.toHaveBeenCalled();
    release();
    await flush();
    expect(liveEntries.size).toBe(0);
  },
);

it('keeps a pending request with a live owner during cache churn', async () => {
  vi.useFakeTimers();
  const api = metricsClient(true);
  api.create.mockResolvedValue(pending());
  const first = prefetchUsageReport(api, defaultSelection, true);
  await flush();
  for (let index = 0; index < 40; index++) {
    const release = prefetchUsageReport(api, { ...defaultSelection, search: String(index) }, true);
    await flush();
    release();
    await flush();
  }
  const second = prefetchUsageReport(api, defaultSelection, true);
  await flush();
  expect(api.create).toHaveBeenCalledTimes(41);
  first();
  expect(liveEntries.size).toBe(1);
  second();
  await flush();
  expect(liveEntries.size).toBe(0);
});

it('keeps an unexpired abandoned pending descriptor resumable', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  const api = metricsClient(true);
  api.create.mockResolvedValue(pending());
  const release = prefetchUsageReport(api, defaultSelection, true);
  await flush();
  release();
  await flush();
  const mounted = renderHook(() => useUsageReport(api, defaultSelection, true));
  await act(async () => vi.advanceTimersByTimeAsync(1000));
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(api.report).toHaveBeenCalledTimes(1);
  expect(mounted.result.current.report).toBeDefined();
});

it.each([false, true])(
  'preserves the retry floor across rapid hide/show (ready=%s)',
  async (ready) => {
    vi.useFakeTimers();
    vi.spyOn(Math, 'random').mockReturnValue(0);
    const api = metricsClient(true);
    const latest = vi
      .fn()
      .mockResolvedValueOnce(
        ready
          ? view()
          : {
              ...view(),
              report: { ...pending(), retry_after_seconds: 2 },
              generated_at: null,
              stale: false,
            },
      )
      .mockResolvedValue(view('idle'));
    api.report.mockImplementation(async (id: string) => ({
      ...reportFixture(true),
      report_id: id,
    }));
    Object.assign(api, { latest });
    const hook = renderHook(({ active }) => useUsageReport(api, defaultSelection, true, active), {
      initialProps: { active: true },
    });
    await flush();
    expect(hook.result.current.busy).toBe(!ready);
    await act(async () => vi.advanceTimersByTimeAsync(250));
    hook.rerender({ active: false });
    hook.rerender({ active: true });
    await flush();
    await act(async () => vi.advanceTimersByTimeAsync(1749));
    expect(latest).toHaveBeenCalledTimes(1);
    act(() => hook.result.current.refresh());
    await act(async () => vi.advanceTimersByTimeAsync(1));
    expect(latest).toHaveBeenCalledTimes(2);
    expect(latest.mock.calls[1]![0].refresh).toBe(false);
    expect(hook.result.current.report).toBeDefined();
    expect(liveEntries.size).toBe(0);
  },
);

it('pauses and resumes an actual document visibility change', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  const visibility = vi.spyOn(document, 'visibilityState', 'get');
  visibility.mockReturnValue('visible');
  const api = metricsClient(true);
  const latest = vi.fn().mockResolvedValueOnce(view()).mockResolvedValue(view('idle'));
  api.report.mockResolvedValue({ ...reportFixture(true), report_id: 'replacement' });
  Object.assign(api, { latest });
  renderHook(() => useUsageReport(api, defaultSelection, true));
  await flush();
  act(() => {
    visibility.mockReturnValue('hidden');
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await act(async () => vi.advanceTimersByTimeAsync(3000));
  expect(latest).toHaveBeenCalledTimes(1);
  act(() => {
    visibility.mockReturnValue('visible');
    document.dispatchEvent(new Event('visibilitychange'));
  });
  await flush();
  expect(latest).toHaveBeenCalledTimes(2);
});

it.each(['principal', 'admin', '403'])(
  'clears two mounted selections on %s mismatch and cancels sibling polling',
  async (kind) => {
    vi.useFakeTimers();
    const api = metricsClient();
    const owner = { principal: 'owner-1', cacheKey: 'epoch-1' };
    const selection = { ...defaultSelection, sandbox: 'second' };
    const first = renderHook(() => useUsageReport(api, defaultSelection, false, true, owner));
    const second = renderHook(() => useUsageReport(api, selection, false, true, owner));
    await flush();
    expect(first.result.current.report).toBeDefined();
    expect(second.result.current.report).toBeDefined();
    api.create.mockResolvedValueOnce({
      ...reportFixture(),
      status: 'pending',
      summary: null,
      retry_after_seconds: 2,
    });
    act(() => second.result.current.refresh());
    await flush();
    if (kind === '403') api.create.mockRejectedValueOnce(new ApiError(403, 'Refused'));
    else
      api.create.mockResolvedValueOnce({
        ...reportFixture(),
        scope: { principal: kind === 'principal' ? 'other' : 'owner-1', admin: kind === 'admin' },
      });
    act(() => first.result.current.refresh());
    await flush();
    expect(first.result.current.report).toBeUndefined();
    expect(second.result.current.report).toBeUndefined();
    expect(second.result.current.error?.denied).toBe(true);
    await act(async () => vi.advanceTimersByTimeAsync(60000));
    expect(second.result.current.report).toBeUndefined();
    expect(api.create).toHaveBeenCalledTimes(4);
    expect(api.report).not.toHaveBeenCalled();
    expect(liveEntries.size).toBe(0);
  },
);

it.each(['principal', 'capabilities', 'epoch'])(
  'keeps opaque identity transitions isolated across a %s change',
  async (change) => {
    const api = metricsClient();
    api.create.mockResolvedValueOnce({
      ...reportFixture(),
      scope: { principal: 'realm:alice', admin: false },
    });
    const initialProps = { principal: 'realm:alice', capabilities: ['read:own'], epoch: 1 };
    const hook = renderHook(
      (props) => {
        const owner = useReportOwner(props.principal, props.capabilities, props.epoch);
        return useUsageReport(api, defaultSelection, false, true, owner);
      },
      { initialProps },
    );
    await flush();
    expect(hook.result.current.report?.scope.principal).toBe('realm:alice');
    api.create.mockRejectedValueOnce(new ApiError(503, 'Unavailable'));
    hook.rerender({
      ...initialProps,
      ...(change === 'principal' ? { principal: 'realm:alice:read' } : {}),
      ...(change === 'capabilities' ? { capabilities: [] } : {}),
      ...(change === 'epoch' ? { epoch: 2 } : {}),
    });
    expect(hook.result.current.report).toBeUndefined();
    await flush();
    expect(hook.result.current.report).toBeUndefined();
    expect(hook.result.current.error).toBeUndefined();
    expect(hook.result.current.retrying).toBe(true);
  },
);

it('does not reset a 429 deadline when idle cache keys are evicted', async () => {
  vi.useFakeTimers();
  const api = metricsClient();
  api.create.mockRejectedValueOnce(new ApiError(429, 'Busy', undefined, undefined, 60000));
  for (let index = 0; index < 40; index++) {
    const release = prefetchUsageReport(api, { ...defaultSelection, search: String(index) }, false);
    await flush();
    release();
  }
  const release = prefetchUsageReport(api, { ...defaultSelection, search: '0' }, false);
  await flush();
  expect(api.create).toHaveBeenCalledTimes(1);
  release();
});
