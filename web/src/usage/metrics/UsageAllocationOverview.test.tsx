import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { UsageAllocationOverview } from './UsageAllocationOverview';
import { MetricsDashboard } from './MetricsDashboard';
import { defaultSelection } from './model';
import { metricRow, metricsClient, reportFixture } from './testFixtures';

afterEach(() => vi.useRealTimers());

function card(label: string) {
  return within(screen.getByText(label, { selector: 'dt' }).parentElement!);
}

for (const projected of [0, 7200]) {
  it.each([
    [true, 0, 'History coverage verified', 'None'],
    [false, 0, 'History not fully verified', 'None confirmed'],
    [false, 3600, 'History has confirmed gaps', '1h'],
    [true, 3600, 'History has confirmed gaps', '1h'],
  ] as const)(
    `keeps coverage complete=%s missing=%s separate from projected seconds=${projected}`,
    (complete, unknown, title, missing) => {
      render(
        <UsageAllocationOverview
          value={{
            ...metricRow.allocated_period,
            cpu_core_hours: 41.5,
            complete,
            unknown_seconds: unknown,
            projected_tail_seconds: projected,
          }}
        />,
      );
      expect(screen.getByRole('heading', { name: title })).toBeVisible();
      expect(card('Reported allocation').getAllByRole('definition')[0]).toHaveTextContent(
        '41.5 core-h',
      );
      expect(card('Known gaps').getAllByRole('definition')[0]).toHaveTextContent(missing);
      expect(card('Projected tail').getAllByRole('definition')[0]).toHaveTextContent(
        projected ? '2.0 h' : '0.0 h',
      );
      expect(
        screen.getByText(/Allocation totals and projected duration use different units/),
      ).toBeVisible();
      expect(
        screen.queryByText(/43.5|Confirmed.*41.5|Never billed as fact/),
      ).not.toBeInTheDocument();
      if (unknown > 0)
        expect(screen.getByRole('note')).toHaveTextContent(
          'Missing intervals are excluded from these totals; this does not mean zero usage.',
        );
      else if (!complete)
        expect(screen.getByRole('note')).toHaveTextContent('That doesn’t mean anything is missing');
    },
  );
}

it('keeps absent allocation unavailable instead of manufacturing a zero or verified history', () => {
  render(<UsageAllocationOverview value={null} />);
  expect(screen.getByRole('heading')).toHaveTextContent('History coverage unavailable');
  for (const label of ['Reported allocation', 'Known gaps', 'Projected tail'])
    expect(card(label).getAllByRole('definition')[0]).toHaveTextContent(/^Unavailable$/);
  expect(screen.queryByText(/0\.0|None confirmed|Coverage verified/)).not.toBeInTheDocument();
});

it.each([
  [1, '<0.001 h'],
  [3600, '1.0 h'],
  ['9007199254740993', '2,501,999,792,983.609 h'],
] as const)(
  'displays projected seconds %s without zeroing or integer precision loss',
  (seconds, text) => {
    render(
      <UsageAllocationOverview
        value={{
          ...metricRow.allocated_period,
          cpu_core_hours: '9007199254740993',
          projected_tail_seconds: seconds,
        }}
      />,
    );
    expect(card('Reported allocation').getAllByRole('definition')[0]).toHaveTextContent(
      '9,007,199,254,740,993 core-h',
    );
    expect(card('Projected tail').getAllByRole('definition')[0]).toHaveTextContent(text);
  },
);

function dashboard(api = metricsClient(), admin = false, measured = false, principal = 'owner-1') {
  return (
    <MetricsDashboard
      api={api}
      history={{ events: vi.fn() }}
      admin={admin}
      measured={measured}
      principal={{ cacheKey: principal, principal }}
      selection={defaultSelection}
      onSelection={vi.fn()}
    />
  );
}

for (const admin of [false, true]) {
  it.each([false, true])(
    `selects the new summary only for personal usage (admin=${admin}, measured=%s)`,
    async (measured) => {
      const api = metricsClient(admin);
      render(dashboard(api, admin, measured));
      await screen.findByRole('button', { name: 'Research Sandbox' });
      const overview = screen.queryByRole('region', { name: 'Allocation and history overview' });
      expect(Boolean(overview)).toBe(!admin);
      if (!admin) {
        expect(within(overview!).getByRole('note')).toHaveTextContent('History has confirmed gaps');
        expect(card('Reported allocation').getAllByRole('definition')[0]).toHaveTextContent(
          '24 core-h',
        );
      }
      expect(screen.getByRole('region', { name: 'Allocated resource-hours' })).toBeInTheDocument();
      expect(
        screen.getByRole('table', {
          name: measured ? 'Sandboxes and measured usage' : 'Sandboxes and allocated usage',
        }),
      ).toBeInTheDocument();
      expect(api.create).toHaveBeenCalledTimes(1);
      expect(api.sandboxes).toHaveBeenCalledTimes(1);
      expect(api.series).toHaveBeenCalledTimes(measured ? (admin ? 3 : 1) : 0);
      if (measured) {
        const disk = screen.getByRole('region', { name: 'Home PVC disk' });
        expect(within(disk).getByText('Unavailable')).toBeVisible();
      }
    },
  );
}

it('does not show zero cards while loading, then keeps the old totals during an explicit refresh', async () => {
  const api = metricsClient();
  let resolve!: (value: ReturnType<typeof reportFixture>) => void;
  api.create.mockReturnValueOnce(
    new Promise((done) => {
      resolve = done;
    }),
  );
  render(dashboard(api));
  expect(screen.getByRole('status')).toHaveTextContent('Loading usage report');
  expect(screen.queryByText('Reported allocation', { selector: 'dt' })).not.toBeInTheDocument();
  await act(async () => resolve(reportFixture()));
  expect(card('Reported allocation').getAllByRole('definition')[0]).toHaveTextContent('24 core-h');
  api.create.mockReturnValueOnce(
    new Promise((done) => {
      resolve = done;
    }),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Refresh usage' }));
  expect(screen.getByText(/As of.*refreshing/)).toBeVisible();
  expect(card('Reported allocation').getAllByRole('definition')[0]).toHaveTextContent('24 core-h');
  const report = reportFixture(false, 'refreshed-report');
  await act(async () =>
    resolve({
      ...report,
      summary: {
        ...report.summary!,
        allocated_period: { ...metricRow.allocated_period, cpu_core_hours: 41.5 },
      },
    }),
  );
  expect(card('Reported allocation').getAllByRole('definition')[0]).toHaveTextContent(
    '41.5 core-h',
  );
  expect(api.create).toHaveBeenCalledTimes(2);
});

it.each([403, 429, 503])('keeps error %s out of the allocation cards', async (status) => {
  vi.useFakeTimers();
  const api = metricsClient();
  api.create.mockRejectedValue(new ApiError(status, 'Fixture refusal', undefined, undefined, 1000));
  render(dashboard(api));
  await act(async () => vi.advanceTimersByTimeAsync(10000));
  expect(screen.getByRole('alert')).toBeInTheDocument();
  expect(screen.queryByText('Reported allocation', { selector: 'dt' })).not.toBeInTheDocument();
  expect(screen.queryByText('None confirmed')).not.toBeInTheDocument();
  expect(api.sandboxes).not.toHaveBeenCalled();
});

it('removes the previous account totals before a late refresh resolves', async () => {
  const api = metricsClient();
  const view = render(dashboard(api));
  await screen.findByText('Reported allocation', { selector: 'dt' });
  let resolve!: (value: ReturnType<typeof reportFixture>) => void;
  api.create.mockReturnValueOnce(
    new Promise((done) => {
      resolve = done;
    }),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Refresh usage' }));
  api.create.mockReturnValue(new Promise(() => {}));
  view.rerender(dashboard(api, false, false, 'owner-2'));
  expect(screen.queryByText('Reported allocation', { selector: 'dt' })).not.toBeInTheDocument();
  await act(async () => resolve(reportFixture()));
  expect(screen.queryByText('Reported allocation', { selector: 'dt' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Research Sandbox' })).not.toBeInTheDocument();
});
