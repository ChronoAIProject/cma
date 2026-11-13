import { act, fireEvent, render, renderHook, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { defaultSelection } from './model';
import { metricsClient } from './testFixtures';
import { useUsageRead, useUsageReport } from './useReport';
import { FilterOptionPicker } from './FilterOptionPicker';

afterEach(() => vi.useRealTimers());

it('preserves the report cooldown across filters, hide/show and explicit refresh', async () => {
  vi.useFakeTimers();
  const api = metricsClient();
  api.create.mockRejectedValueOnce(new ApiError(429, 'Busy', undefined, undefined, 30000));
  const { result, rerender } = renderHook(
    ({ selection, active }) => useUsageReport(api, selection, false, active),
    { initialProps: { selection: defaultSelection, active: true } },
  );
  await act(async () => {});
  const deadline = result.current.error?.retryAt;
  await act(async () => vi.advanceTimersByTimeAsync(5000));
  const selection = { ...defaultSelection, search: 'Research' };
  rerender({ selection, active: true });
  await act(async () => {});
  expect(result.current.error?.retryAt).toBe(deadline);
  rerender({ selection, active: false });
  rerender({ selection, active: true });
  act(() => result.current.refresh());
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(result.current.report).toBeUndefined();
  await act(async () => vi.advanceTimersByTimeAsync(25000));
  expect(api.create).toHaveBeenCalledTimes(1);
  act(() => result.current.refresh());
  await act(async () => {});
  expect(api.create).toHaveBeenCalledTimes(2);
  expect(api.create.mock.calls[1]![0].filters.search).toBe('Research');
  expect(result.current.error).toBeUndefined();
  expect(result.current.report).toBeDefined();
});

it('retains the previous report honestly when a new selection hits backoff', async () => {
  vi.useFakeTimers();
  const api = metricsClient();
  const { result, rerender } = renderHook(
    ({ selection }) => useUsageReport(api, selection, false),
    { initialProps: { selection: defaultSelection } },
  );
  await act(async () => {});
  api.create.mockRejectedValueOnce(new ApiError(429, 'Busy'));
  rerender({ selection: { ...defaultSelection, search: 'first' } });
  await act(async () => {});
  rerender({ selection: { ...defaultSelection, search: 'second' } });
  await act(async () => {});
  expect(api.create).toHaveBeenCalledTimes(2);
  expect(result.current.error?.title).toBe('Usage reporting is busy');
  expect(result.current.report?.report_id).toBe('report-one');
  expect(result.current.stale).toBe(true);
});

it('shares a failed member read with discovery, including remounts and rapid retries', async () => {
  vi.useFakeTimers();
  const api = metricsClient();
  const load = vi.fn().mockRejectedValue(new ApiError(429, 'Busy'));
  const read = renderHook(() => useUsageRead(load, true, api));
  await act(async () => {});
  const deadline = read.result.current.error?.retryAt;
  act(() => read.result.current.retry());
  await act(async () => {});
  expect(load).toHaveBeenCalledTimes(1);
  read.unmount();
  const view = render(
    <FilterOptionPicker api={api} kind="owner" value="" onChange={() => {}} active />,
  );
  fireEvent.click(screen.getByRole('button', { name: 'Owner: Everyone' }));
  await act(async () => {});
  expect(screen.getByText('Usage reporting is busy')).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Waiting to retry' })).toBeDisabled();
  expect(api.options).not.toHaveBeenCalled();
  fireEvent.change(screen.getByRole('textbox', { name: 'Search owner choices' }), {
    target: { value: 'Research' },
  });
  await act(async () => vi.advanceTimersByTimeAsync(500));
  expect(api.options).not.toHaveBeenCalled();
  await act(async () => vi.advanceTimersByTimeAsync(deadline! - Date.now()));
  expect(api.options).not.toHaveBeenCalled();
  api.options.mockRejectedValueOnce(new ApiError(503, 'Unavailable'));
  fireEvent.click(screen.getByRole('button', { name: 'Retry choices' }));
  await act(async () => {});
  expect(screen.getByText('Usage reporting is unavailable')).toBeInTheDocument();
  expect(screen.queryByText('No matching owner choices.')).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Retry choices' }));
  await act(async () => {});
  expect(screen.getByText('No matching owner choices.')).toBeInTheDocument();
  expect(api.options).toHaveBeenCalledTimes(2);
  view.unmount();
});
