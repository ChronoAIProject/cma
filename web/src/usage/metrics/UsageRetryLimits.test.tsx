import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { defaultSelection } from './model';
import { metricsClient, reportFixture } from './testFixtures';
import { useUsageRead, useUsageReport } from './useReport';

const flush = () => act(async () => {});
function clock() {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
}
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('times out hung transports, cancels each request, and stops at the total observation bound', async () => {
  clock();
  const api = metricsClient();
  api.create.mockImplementation(() => new Promise(() => {}));
  const hook = renderHook(() => useUsageReport(api, defaultSelection, false));
  await flush();
  await act(async () => vi.advanceTimersByTimeAsync(29999));
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(api.create.mock.calls[0]![1].signal.aborted).toBe(false);
  await act(async () => vi.advanceTimersByTimeAsync(1001));
  expect(api.create.mock.calls[0]![1].signal.aborted).toBe(true);
  expect(api.create).toHaveBeenCalledTimes(2);
  await act(async () => vi.advanceTimersByTimeAsync(89000));
  expect(api.create).toHaveBeenCalledTimes(4);
  expect(api.create.mock.calls.every(([, options]) => options.signal.aborted)).toBe(true);
  expect(hook.result.current.error?.title).toContain('taking longer');
  await act(async () => vi.advanceTimersByTimeAsync(120000));
  expect(api.create).toHaveBeenCalledTimes(4);
});

it('does not send a request when returning after the observation budget elapsed while hidden', async () => {
  clock();
  const api = metricsClient();
  api.create.mockResolvedValue({ ...reportFixture(), status: 'pending', summary: null });
  const hook = renderHook(({ active }) => useUsageReport(api, defaultSelection, false, active), {
    initialProps: { active: true },
  });
  await flush();
  hook.rerender({ active: false });
  await act(async () => vi.advanceTimersByTimeAsync(121000));
  hook.rerender({ active: true });
  await flush();
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(api.report).not.toHaveBeenCalled();
  expect(hook.result.current.error?.title).toContain('taking longer');
});

it('preserves a long pending floor across bounded observation and a deliberate retry', async () => {
  clock();
  const api = metricsClient();
  api.create.mockResolvedValueOnce({
    ...reportFixture(),
    status: 'pending',
    summary: null,
    retry_after_seconds: 180,
  });
  const hook = renderHook(() => useUsageReport(api, defaultSelection, false));
  await flush();
  await act(async () => vi.advanceTimersByTimeAsync(120000));
  expect(hook.result.current.stopped).toBe(true);
  act(() => hook.result.current.refresh());
  await flush();
  await act(async () => vi.advanceTimersByTimeAsync(59999));
  expect(api.create).toHaveBeenCalledTimes(1);
  await act(async () => vi.advanceTimersByTimeAsync(1));
  expect(api.create).toHaveBeenCalledTimes(2);
  expect(hook.result.current.report).toBeDefined();
});

it('recovers child reads automatically after the dependency Retry-After floor', async () => {
  clock();
  const api = metricsClient();
  const load = vi
    .fn()
    .mockRejectedValueOnce(new ApiError(503, 'Unavailable', undefined, undefined, 5000))
    .mockResolvedValue('page');
  const hook = renderHook(() => useUsageRead(load, true, api));
  await flush();
  expect(hook.result.current.error).toBeUndefined();
  await act(async () => vi.advanceTimersByTimeAsync(4999));
  expect(load).toHaveBeenCalledTimes(1);
  await act(async () => vi.advanceTimersByTimeAsync(1));
  expect(load).toHaveBeenCalledTimes(2);
  expect(hook.result.current.data).toBe('page');
});
