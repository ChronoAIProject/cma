import { act, fireEvent, render, renderHook, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, ProtocolError, TransportError } from '@cma/client';
import { defaultSelection, type Selection } from './model';
import { asOf, metricRow, metricsClient, reportFixture } from './testFixtures';
import { useUsageReport } from './useReport';
import { MetricsDashboard } from './MetricsDashboard';

const flush = () => act(async () => {});
const selected: Selection = { ...defaultSelection, range: '30d' };
const owner = { principal: 'owner-1', cacheKey: 'owner-1/epoch-1' };
const view = (status: 'ready' | 'pending', id = 'job-one') => ({
  report: {
    ...reportFixture(),
    report_id: id,
    status,
    summary: status === 'ready' ? reportFixture().summary : null,
  },
  generated_at: status === 'ready' ? asOf : null,
  stale: false,
  refresh: {
    state: status === 'ready' ? 'idle' : 'pending',
    report_id: status === 'ready' ? null : id,
    retry_after_seconds: 2,
  },
});
function clock() {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
}
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it.each([
  new ApiError(503, 'Unavailable', undefined, undefined, 4000),
  new ApiError(429, 'Busy', undefined, undefined, 4000),
  new TransportError('Disconnected'),
])('recovers a transient report failure without publishing an error', async (failure) => {
  clock();
  const api = metricsClient();
  api.create.mockRejectedValueOnce(failure);
  const hook = renderHook(() => useUsageReport(api, selected, false, true, owner));
  await flush();
  expect(hook.result.current.retrying).toBe(true);
  expect(hook.result.current.error).toBeUndefined();
  const floor = failure instanceof ApiError ? 4000 : 1000;
  act(() => hook.result.current.refresh());
  await act(async () => vi.advanceTimersByTimeAsync(floor - 1));
  expect(api.create).toHaveBeenCalledTimes(1);
  await act(async () => vi.advanceTimersByTimeAsync(1));
  expect(api.create).toHaveBeenCalledTimes(2);
  expect(hook.result.current.report).toBeDefined();
  expect(hook.result.current.error).toBeUndefined();
});

it('exhausts four attempts, stops periodic resubmission, and allows deliberate retry', async () => {
  clock();
  const api = metricsClient();
  api.create.mockRejectedValue(new ApiError(503, 'Unavailable'));
  const hook = renderHook(() => useUsageReport(api, selected, false, true, owner));
  await flush();
  await act(async () => vi.advanceTimersByTimeAsync(7000));
  expect(api.create).toHaveBeenCalledTimes(4);
  expect(hook.result.current.error?.title).toBe('Usage reporting is unavailable');
  expect(hook.result.current.stopped).toBe(true);
  await act(async () => vi.advanceTimersByTimeAsync(180000));
  expect(api.create).toHaveBeenCalledTimes(4);
  api.create.mockResolvedValue(reportFixture());
  act(() => hook.result.current.refresh());
  await flush();
  expect(hook.result.current.report).toBeDefined();
});

it('preserves the seven-day allocation resource and units after one successful Retry', async () => {
  clock();
  const api = metricsClient();
  render(
    <MetricsDashboard
      api={api}
      history={{ events: vi.fn() }}
      admin={false}
      principal={owner}
      selection={{ ...defaultSelection, range: '7d' }}
      onSelection={() => {}}
    />,
  );
  await flush();
  const chart = () => within(screen.getByRole('region', { name: 'Allocation comparison chart' }));
  fireEvent.click(chart().getByRole('button', { name: 'Allocation chart resource' }));
  fireEvent.click(screen.getByRole('menuitemradio', { name: 'Memory' }));
  expect(chart().getByText('Memory · GiB-hours in the report period')).toBeVisible();

  api.create.mockRejectedValue(new TransportError('Disconnected'));
  fireEvent.click(screen.getByRole('button', { name: 'Refresh usage' }));
  await flush();
  await act(async () => vi.advanceTimersByTimeAsync(7000));
  expect(screen.getByText('Could not load usage')).toBeVisible();

  api.create.mockResolvedValue(reportFixture(false, 'recovered-report'));
  await act(async () => vi.advanceTimersByTimeAsync(8000));
  const attemptsBeforeRetry = api.create.mock.calls.length;
  fireEvent.click(screen.getByRole('button', { name: 'Retry usage' }));
  await flush();

  expect(screen.queryByText('Could not load usage')).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Usage time range' })).toHaveTextContent('Last 7 days');
  expect(api.create).toHaveBeenCalledTimes(attemptsBeforeRetry + 1);
  expect(api.sandboxes).toHaveBeenLastCalledWith('recovered-report', expect.anything());
  expect(chart().getByRole('button', { name: 'Allocation chart resource' })).toHaveTextContent(
    'Memory',
  );
  expect(chart().getByRole('img')).toHaveAccessibleName('Memory allocated GiB-hours by sandbox');
});

it.each([400, 401, 403, 404, 410])(
  'does not retry permanent %s or admit another latest job',
  async (status) => {
    clock();
    const api = metricsClient();
    const latest = vi.fn().mockResolvedValue(view('pending'));
    Object.assign(api, { latest });
    api.report.mockRejectedValue(new ApiError(status, 'Refused', 'report_expired'));
    const hook = renderHook(() => useUsageReport(api, selected, false, true, owner));
    await flush();
    await act(async () => vi.advanceTimersByTimeAsync(180000));
    expect(latest).toHaveBeenCalledTimes(1);
    expect(api.report).toHaveBeenCalledTimes(1);
    expect(hook.result.current.error).toBeDefined();
    expect(hook.result.current.busy).toBe(false);
  },
);

it('never retries malformed report responses', async () => {
  clock();
  const api = metricsClient();
  api.create.mockRejectedValue(new ProtocolError('Malformed'));
  const hook = renderHook(() => useUsageReport(api, selected, false));
  await flush();
  await act(async () => vi.advanceTimersByTimeAsync(120000));
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(hook.result.current.error).toBeDefined();
});

it('polls the admitted ID and replaces the entire snapshot after ready', async () => {
  clock();
  const api = metricsClient();
  const latest = vi
    .fn()
    .mockResolvedValueOnce(view('pending'))
    .mockResolvedValue(view('ready', 'newer-committed'));
  Object.assign(api, { latest });
  api.report.mockResolvedValue({ ...reportFixture(), report_id: 'job-one' });
  const hook = renderHook(() => useUsageReport(api, selected, false, true, owner));
  await flush();
  await act(async () => vi.advanceTimersByTimeAsync(2000));
  expect(api.report).toHaveBeenCalledWith('job-one', expect.anything());
  expect(latest).toHaveBeenCalledTimes(2);
  expect(latest.mock.calls[1]![0].refresh).toBe(false);
  expect(hook.result.current.report?.report_id).toBe('newer-committed');
});

it('stops an endlessly pending job after two minutes without readmitting it', async () => {
  clock();
  const api = metricsClient();
  const latest = vi.fn().mockResolvedValue(view('pending'));
  Object.assign(api, { latest });
  api.report.mockResolvedValue({ ...view('pending').report, retry_after_seconds: 2 });
  const hook = renderHook(() => useUsageReport(api, selected, false, true, owner));
  await flush();
  await act(async () => vi.advanceTimersByTimeAsync(120000));
  expect(hook.result.current.error?.title).toContain('taking longer');
  expect(hook.result.current.busy).toBe(false);
  const calls = api.report.mock.calls.length;
  await act(async () => vi.advanceTimersByTimeAsync(120000));
  expect(api.report).toHaveBeenCalledTimes(calls);
  expect(latest).toHaveBeenCalledTimes(1);
});

it('keeps the retry floor and prevents late publication across hide/show', async () => {
  clock();
  const api = metricsClient();
  api.create.mockRejectedValueOnce(new ApiError(503, 'Unavailable', undefined, undefined, 4000));
  const hook = renderHook(({ active }) => useUsageReport(api, selected, false, active, owner), {
    initialProps: { active: true },
  });
  await flush();
  await act(async () => vi.advanceTimersByTimeAsync(500));
  hook.rerender({ active: false });
  await act(async () => vi.advanceTimersByTimeAsync(500));
  hook.rerender({ active: true });
  await act(async () => vi.advanceTimersByTimeAsync(2999));
  expect(api.create).toHaveBeenCalledTimes(1);
  await act(async () => vi.advanceTimersByTimeAsync(1));
  expect(api.create).toHaveBeenCalledTimes(2);
  expect(hook.result.current.report).toBeDefined();
});

it('labels a same-owner nearest range and fetches children using its exact report ID', async () => {
  clock();
  const api = metricsClient();
  const warmed = renderHook(() => useUsageReport(api, defaultSelection, false, true, owner));
  await flush();
  warmed.unmount();
  api.create.mockResolvedValue({
    ...reportFixture(),
    report_id: 'month-job',
    status: 'pending',
    summary: null,
  });
  api.sandboxes.mockResolvedValue({
    report_id: 'report-one',
    as_of: asOf,
    items: [metricRow],
    next: null,
    total_count: 1,
  });
  const rendered = render(
    <MetricsDashboard
      api={api}
      history={{ events: vi.fn() }}
      admin={false}
      principal={owner}
      selection={selected}
      onSelection={() => {}}
    />,
  );
  await flush();
  expect(screen.getByText('Showing Last 24 hours')).toBeInTheDocument();
  expect(screen.getByText(/not last 30 days/)).toBeInTheDocument();
  expect(api.sandboxes).toHaveBeenCalledWith('report-one', expect.anything());
  expect(rendered.container.querySelector('.usage-loading')).not.toBeInTheDocument();
  expect(screen.getByText(/completion estimate is not available/)).toBeInTheDocument();
  expect(rendered.container).toHaveTextContent('Capacity reserved over the report period.');
  expect(rendered.container).toHaveTextContent('Allocated resource-hours for the report period');
  expect(rendered.container).toHaveTextContent('core-hours in the report period');
  expect(rendered.container).not.toHaveTextContent(/selected period/i);
  fireEvent.click(screen.getByRole('button', { name: 'Research Sandbox' }));
  await flush();
  expect(api.sandbox).toHaveBeenCalledWith('report-one', 'sbx-1', expect.anything());
  expect(screen.getByRole('region', { name: 'Sandbox usage details' })).toHaveTextContent(
    'Capacity reserved over the report period.',
  );
  expect(rendered.container).not.toHaveTextContent(/selected period/i);
  fireEvent.click(screen.getByRole('button', { name: 'Refresh usage' }));
  expect(api.create).toHaveBeenCalledTimes(2);
});

it.each(['owner', 'membership', 'expiry'])(
  'does not preview a warmed range across %s boundaries',
  async (boundary) => {
    clock();
    const api = metricsClient();
    const first = renderHook(() => useUsageReport(api, defaultSelection, false, true, owner));
    await flush();
    first.unmount();
    if (boundary === 'expiry') await act(async () => vi.advanceTimersByTimeAsync(300001));
    api.create.mockRejectedValue(new ApiError(503, 'Unavailable'));
    const hook = renderHook(() =>
      useUsageReport(
        api,
        boundary === 'membership' ? { ...selected, search: 'other' } : selected,
        false,
        true,
        boundary === 'owner' ? { principal: 'owner-2', cacheKey: 'owner-2/epoch' } : owner,
      ),
    );
    expect(hook.result.current.fallback).toBeUndefined();
    await flush();
    expect(hook.result.current.fallback).toBeUndefined();
  },
);
