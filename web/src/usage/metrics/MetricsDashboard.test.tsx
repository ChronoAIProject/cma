import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { MetricsDashboard } from './MetricsDashboard';
import { defaultSelection } from './model';
import { asOf, metricRow, metricsClient, reportFixture } from './testFixtures';

// Recharts geometry is checked in the local browser fixture; these tests prove API orchestration.
vi.mock('./MetricChart', () => ({
  MetricChart: ({ series }: { series: { metric: string; report_id: string } }) => (
    <section aria-label={`${series.metric} chart`} data-report={series.report_id} />
  ),
}));
function renderDashboard(api = metricsClient(true), admin = true) {
  const onSelection = vi.fn();
  const history = { events: vi.fn().mockResolvedValue({ items: [], next: null }) };
  const view = render(
    <MetricsDashboard
      api={api}
      measured
      history={history}
      admin={admin}
      selection={defaultSelection}
      onSelection={onSelection}
    />,
  );
  return { ...view, api, onSelection, history };
}
it('uses one report for whole-cohort cards, charts, globally sorted 50-row pages and drilldown', async () => {
  const actor = userEvent.setup();
  const { api } = renderDashboard();
  await screen.findByRole('button', { name: 'Research Sandbox' });
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(api.sandboxes).toHaveBeenCalledTimes(1);
  expect(api.sandboxes).toHaveBeenCalledWith(
    'report-one',
    expect.objectContaining({ limit: 50, sort: 'last_activity_at', order: 'desc' }),
  );
  expect(api.series).toHaveBeenCalledTimes(3);
  for (const [id] of api.series.mock.calls) expect(id).toBe('report-one');
  expect(screen.getByText(/25,001 Sandboxes · 18,000 running/)).toBeInTheDocument();
  expect(api.sandbox).not.toHaveBeenCalled();
  api.sandboxes.mockResolvedValueOnce({
    report_id: 'report-one',
    as_of: asOf,
    items: [{ ...metricRow, sandbox_id: 'sbx-2', name: 'Second page' }],
    next: null,
    total_count: 25001,
  });
  await actor.click(screen.getByRole('button', { name: 'Next usage page' }));
  await screen.findByRole('button', { name: 'Second page' });
  expect(api.sandboxes).toHaveBeenCalledTimes(2);
  expect(api.sandboxes).toHaveBeenLastCalledWith(
    'report-one',
    expect.objectContaining({ cursor: 'page-two', limit: 50 }),
  );
  expect(api.create).toHaveBeenCalledTimes(1);
});
it('keeps unmounted disk unknown and exposes last-observed data separately from allocation', async () => {
  renderDashboard();
  await screen.findByRole('button', { name: 'Research Sandbox' });
  const card = screen.getByRole('region', { name: 'Home PVC disk' });
  expect(within(card).getByText(/Unavailable/)).toBeInTheDocument();
  expect(within(card).getByText(/Last observed: 4 GiB/)).toBeInTheDocument();
  expect(screen.getAllByText(/Allocation history: partial/).length).toBeGreaterThan(0);
  expect(screen.queryByText(/charges|credits|price|\$/i)).not.toBeInTheDocument();
});
it('shows a distinct empty selection without pretending a source outage is empty', async () => {
  const api = metricsClient(true);
  const report = reportFixture(true);
  api.create.mockResolvedValue({
    ...report,
    summary: { ...report.summary, sandbox_count: 0, running_count: 0, measured: null },
  });
  api.sandboxes.mockResolvedValue({
    report_id: report.report_id,
    as_of: asOf,
    items: [],
    next: null,
    total_count: 0,
  });
  api.series.mockImplementation(async (id, options) => ({
    report_id: id,
    metric: options.metric,
    unit: 'cores',
    source: 'fixture',
    lines: [],
  }));
  renderDashboard(api);
  await screen.findByText('No Sandboxes match this selection.');
  expect(screen.queryByText('Usage reporting is unavailable')).not.toBeInTheDocument();
});
it.each([410, 429, 503])(
  'shows report failure %s without an empty or zero-usage claim',
  async (status) => {
    const api = metricsClient(true);
    api.create.mockRejectedValue(new ApiError(status, 'Fixture failure'));
    renderDashboard(api);
    await screen.findByRole('alert');
    expect(screen.queryByText('No Sandboxes match this selection.')).not.toBeInTheDocument();
    expect(api.sandboxes).not.toHaveBeenCalled();
    expect(api.series).not.toHaveBeenCalled();
  },
);
it('does not read usage when closed', () => {
  const api = metricsClient(false);
  render(
    <MetricsDashboard
      api={api}
      history={{ events: vi.fn() }}
      admin={false}
      active={false}
      selection={defaultSelection}
      onSelection={vi.fn()}
    />,
  );
  expect(api.create).not.toHaveBeenCalled();
});
it('drops data after authorization is revoked during a page read', async () => {
  const api = metricsClient(true);
  api.sandboxes.mockRejectedValue(new ApiError(403, 'Revoked'));
  renderDashboard(api);
  await waitFor(() =>
    expect(screen.queryByText(/25,001 Sandboxes · 18,000 running/)).not.toBeInTheDocument(),
  );
  await screen.findByRole('alert');
  await waitFor(() =>
    expect(
      screen.queryByRole('table', { name: 'Sandboxes and measured usage' }),
    ).not.toBeInTheDocument(),
  );
});
