import { act, fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { MetricsDashboard } from './MetricsDashboard';
import { defaultSelection, type Selection } from './model';
import { metricRow, metricsClient, reportFixture } from './testFixtures';

afterEach(() => vi.useRealTimers());

vi.mock('./MetricChart', () => ({
  MetricChart: ({ series }: { series: { metric: string } }) => (
    <section aria-label={`${series.metric} chart`} />
  ),
}));

function setup(
  admin = false,
  selection: Selection = defaultSelection,
  api = metricsClient(admin),
  measured?: boolean,
) {
  const history = { events: vi.fn().mockResolvedValue({ items: [], next: null }) };
  render(
    <MetricsDashboard
      api={api}
      measured={measured}
      history={history}
      admin={admin}
      selection={selection}
      onSelection={vi.fn()}
    />,
  );
  return { api, history, actor: userEvent.setup() };
}

it.each([false, true])(
  'defaults to allocation for admin=%s, including detail and lifecycle history',
  async (admin) => {
    const api = metricsClient(admin);
    const report = reportFixture(admin);
    api.create.mockResolvedValue({
      ...report,
      source_health: 'source_unavailable',
      report_complete: false,
      warnings: [
        'source_unavailable',
        'measurement_coverage_incomplete',
        'allocation_history_incomplete',
      ],
    });
    const { actor, history } = setup(admin, defaultSelection, api);
    await screen.findByRole('button', { name: 'Research Sandbox' });
    const capacity = screen.getByRole('region', { name: 'Allocated capacity now' });
    for (const text of ['2 cores', '4 GiB', '20 GiB'])
      expect(within(capacity).getByText(text)).toBeInTheDocument();
    const period = screen.getByRole('region', { name: 'Allocated resource-hours' });
    for (const text of ['24', '48', '480'])
      expect(within(period).getByText(text)).toBeInTheDocument();
    expect(within(period).getByText(/Missing intervals confirmed/)).toBeInTheDocument();
    expect(screen.getByText(/Allocation period:/)).toBeInTheDocument();
    expect(
      screen.queryByText(
        /source_unavailable|measurement_coverage|Measured through|Measured coverage|report is incomplete/i,
      ),
    ).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Chart grouping' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Memory working set' })).not.toBeInTheDocument();
    expect(
      screen.queryByRole('region', { name: 'Measured resource charts' }),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('columnheader', { name: /mean \/ allocated|Disk used/ }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText(/Infrastructure diagnostics/)).not.toBeInTheDocument();
    const table = screen.getByRole('table', { name: 'Sandboxes and allocated usage' });
    expect(
      within(table).getByRole('columnheader', { name: /CPU \(core-hours\)/ }),
    ).toBeInTheDocument();
    expect(api.series).not.toHaveBeenCalled();
    await actor.click(screen.getByRole('button', { name: 'Research Sandbox' }));
    const detail = screen.getByRole('region', { name: 'Sandbox usage details' });
    await within(detail).findByRole('region', { name: 'Allocated capacity now' });
    expect(
      within(detail).getByRole('region', { name: 'Allocated resource-hours' }),
    ).toBeInTheDocument();
    expect(api.sandbox).toHaveBeenCalledWith('report-one', 'sbx-1', expect.anything());
    expect(api.series).not.toHaveBeenCalled();
    await actor.click(within(detail).getByRole('button', { name: 'Lifecycle history' }));
    await within(detail).findByRole('table', { name: 'Usage intervals' });
    expect(history.events).toHaveBeenCalled();
    expect(
      screen.queryByText(/Measured coverage|Period mean|Last observed/),
    ).not.toBeInTheDocument();
  },
);

it('uses singular Agent only for a count of one in the operations Usage inventory', async () => {
  const api = metricsClient(true);
  api.sandboxes.mockResolvedValue({
    report_id: 'report-one',
    as_of: 1_700_000_000,
    items: [0, 1, 4].map((count) => ({
      ...metricRow,
      sandbox_id: `sbx-count-${count}`,
      name: `Sandbox ${count}`,
      agent_count: count,
    })),
    next: null,
    total_count: 3,
  });
  setup(true, defaultSelection, api);
  for (const [count, text] of [
    [0, '0 Agents'],
    [1, '1 Agent'],
    [4, '4 Agents'],
  ]) {
    const row = within(await screen.findByRole('row', { name: new RegExp(`Sandbox ${count} `) }));
    expect(row.getByText(`Details · ${text}`, { exact: true })).toBeInTheDocument();
  }
});

it.each(['cpu_mean_cores', 'memory_mean_bytes', 'disk_latest_bytes'] as const)(
  'does not request hidden measured sort %s from an old URL selection',
  async (sort) => {
    const { api } = setup(true, { ...defaultSelection, sort, order: 'asc' });
    await screen.findByRole('button', { name: 'Research Sandbox' });
    expect(api.sandboxes).toHaveBeenCalledWith(
      'report-one',
      expect.objectContaining({ sort: 'last_activity_at', order: 'desc' }),
    );
    expect(api.series).not.toHaveBeenCalled();
  },
);

it.each([false, true])(
  'explicitly enables measured summary, charts and detail for admin=%s',
  async (admin) => {
    const { api, actor } = setup(admin, defaultSelection, metricsClient(admin), true);
    await screen.findByRole('region', { name: 'cpu chart' });
    expect(screen.getByRole('table', { name: 'Sandboxes and measured usage' })).toBeInTheDocument();
    expect(screen.getByRole('region', { name: 'Allocated resource-hours' })).toBeInTheDocument();
    expect(screen.getAllByText(/Measured coverage:/).length).toBe(3);
    if (admin) expect(screen.getByRole('button', { name: 'Chart grouping' })).toBeInTheDocument();
    else await actor.click(screen.getByRole('button', { name: 'Memory working set' }));
    await actor.click(screen.getByRole('button', { name: 'Research Sandbox' }));
    const detail = screen.getByRole('region', { name: 'Sandbox usage details' });
    for (const metric of ['cpu', 'memory', 'disk'])
      await within(detail).findByRole('region', { name: `${metric} chart` });
    expect(api.series).toHaveBeenCalledWith(
      'report-one',
      expect.objectContaining({ group_by: 'sandbox', series_id: 'sbx-1' }),
    );
  },
);

it.each([403, 410, 429, 503])('preserves report failure %s in allocated mode', async (status) => {
  const api = metricsClient();
  vi.useFakeTimers();
  api.create.mockRejectedValue(new ApiError(status, 'Fixture failure', undefined, undefined, 1000));
  setup(false, defaultSelection, api);
  await act(async () => vi.advanceTimersByTimeAsync(10000));
  const notice = screen.getByRole('alert');
  expect(notice).not.toHaveTextContent(/Measured usage/);
  expect(screen.queryByText('No Sandboxes match this selection.')).not.toBeInTheDocument();
  expect(
    screen.queryByRole('region', { name: 'Allocated resource-hours' }),
  ).not.toBeInTheDocument();
  expect(api.series).not.toHaveBeenCalled();
});

it('keeps allocation visible after a detail failure and never requests measured series', async () => {
  vi.useFakeTimers();
  const api = metricsClient();
  api.sandbox.mockRejectedValue(new ApiError(503, 'Fixture detail failure'));
  setup(false, defaultSelection, api);
  await act(async () => {});
  fireEvent.click(screen.getByRole('button', { name: 'Research Sandbox' }));
  await act(async () => vi.advanceTimersByTimeAsync(10000));
  const detail = screen.getByRole('region', { name: 'Sandbox usage details' });
  expect(within(detail).getByRole('alert')).toBeInTheDocument();
  expect(api.sandbox).toHaveBeenCalledTimes(4);
  expect(screen.getByRole('region', { name: 'Allocated resource-hours' })).toBeInTheDocument();
  expect(api.series).not.toHaveBeenCalled();
});
