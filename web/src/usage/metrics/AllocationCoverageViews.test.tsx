import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { AllocationChart } from './AllocationChart';
import { AllocationHistory } from './AllocationHistory';
import { UsageInventoryRow } from './UsageInventoryRow';
import { OwnerAllocationChart } from './OwnerAllocationChart';
import { MetricSummary } from './MetricSummary';
import { asOf, metricRow, metricsClient, reportFixture } from './testFixtures';

const props = {
  api: metricsClient(),
  reportId: 'coverage-report',
  active: true,
  onFailure: vi.fn(),
};
const states = [
  { complete: true, unknown_seconds: '0', label: 'Coverage verified' },
  { complete: false, unknown_seconds: '3600', label: 'Missing intervals confirmed' },
  { complete: false, unknown_seconds: '0', label: 'Coverage unverified' },
] as const;

it('does not imply allocation gaps when only measurement coverage is incomplete', () => {
  const report = reportFixture(false);
  render(
    <MetricSummary
      measured
      report={{
        ...report,
        report_complete: false,
        summary: {
          ...report.summary!,
          allocated_period: {
            ...metricRow.allocated_period,
            complete: false,
            unknown_seconds: '0',
            projected_tail_seconds: '0',
          },
        },
      }}
    />,
  );
  expect(screen.getByRole('status')).not.toHaveTextContent('allocation-history gaps');
  expect(screen.getByRole('status')).toHaveTextContent(
    'Review measurement and allocation coverage',
  );
  expect(screen.getByText('No missing interval has been established.')).toBeVisible();
});

it('does not turn unverified coverage with zero confirmed missing time into a chart gap', () => {
  const row = {
    ...metricRow,
    allocated_period: {
      ...metricRow.allocated_period,
      complete: false,
      unknown_seconds: '0',
      projected_tail_seconds: '0',
    },
  };
  render(
    <AllocationChart
      {...props}
      admin={false}
      page={{ report_id: props.reportId, as_of: asOf, next: null, total_count: 1, items: [row] }}
    />,
  );
  const chart = screen.getByRole('region', { name: 'Allocation comparison chart' });
  expect(chart).not.toHaveTextContent(/Partial history|Missing history is excluded/);
  expect(chart).toHaveTextContent('Coverage unverified');
  expect(chart).toHaveTextContent('No missing interval has been established');
});

for (const state of states) {
  it.each(['0', '120'])(
    `${state.label} with projected seconds=%s agrees across views`,
    async (projected) => {
      const actor = userEvent.setup();
      const period = { ...metricRow.allocated_period, ...state, projected_tail_seconds: projected };
      const row = { ...metricRow, allocated_period: period };
      render(
        <>
          <section aria-label="Report coverage">
            <AllocationHistory value={period} />
          </section>
          <AllocationChart
            {...props}
            admin={false}
            page={{
              report_id: props.reportId,
              as_of: asOf,
              next: null,
              total_count: 1,
              items: [row],
            }}
          />
          <table aria-label="Inventory">
            <tbody>
              <UsageInventoryRow row={row} admin={false} measured={false} onDetail={vi.fn()} />
            </tbody>
          </table>
        </>,
      );
      await actor.click(screen.getByText('View allocation chart data'));
      const views = [
        screen.getByRole('region', { name: 'Report coverage' }),
        screen.getByLabelText('Plotted Sandboxes coverage'),
        await screen.findByRole('table', { name: 'Allocation chart data' }),
        screen.getByRole('table', { name: 'Inventory' }),
      ];
      for (const view of views) {
        expect(view).toHaveTextContent(state.label);
        expect(view).not.toHaveTextContent(/Partial history|Complete history/);
        if (state.label === 'Coverage unverified') {
          expect(view).toHaveTextContent('No missing interval has been established');
          expect(view).not.toHaveTextContent(/Missing intervals are excluded|zero usage/);
        }
        if (state.label === 'Missing intervals confirmed') {
          expect(view).toHaveTextContent('1h');
          expect(view).toHaveTextContent(
            /Missing intervals are excluded.*does not mean zero usage/,
          );
        }
        if (projected !== '0') {
          expect(view).toHaveTextContent('Projection from last known state: 2m (estimated)');
        } else expect(view).not.toHaveTextContent('(estimated)');
      }
      await actor.click(within(views[0]!).getByText('How this is calculated'));
      expect(within(views[0]!).getByText('Missing intervals').nextElementSibling).toHaveTextContent(
        state.label === 'Coverage unverified'
          ? 'None confirmed'
          : state.unknown_seconds === '0'
            ? 'None'
            : '1h',
      );
    },
  );
}

it('keeps report, plotted resource ranking, and off-chart row coverage in their own scopes', async () => {
  const actor = userEvent.setup();
  const items = Array.from({ length: 9 }, (_, index) => ({
    ...metricRow,
    sandbox_id: `scope-${index}`,
    name: `Scope ${index}`,
    allocated_period: {
      ...metricRow.allocated_period,
      complete: index !== 8,
      cpu_core_hours: 9 - index,
      disk_gib_hours: index + 1,
      unknown_seconds: index === 8 ? '3600' : '0',
      projected_tail_seconds: index === 8 ? '120' : '0',
    },
  }));
  render(
    <>
      <section aria-label="Report coverage">
        <AllocationHistory
          value={{
            ...metricRow.allocated_period,
            complete: false,
            unknown_seconds: '0',
            projected_tail_seconds: '0',
          }}
        />
      </section>
      <AllocationChart
        {...props}
        admin={false}
        page={{ report_id: props.reportId, as_of: asOf, next: null, total_count: 100, items }}
      />
    </>,
  );
  expect(screen.getByRole('region', { name: 'Report coverage' })).toHaveTextContent(
    'Coverage unverified',
  );
  const caption = screen.getByLabelText('Plotted Sandboxes coverage');
  expect(caption).toHaveTextContent('Coverage verified');
  expect(caption).not.toHaveTextContent(
    /Missing intervals confirmed|Projection from last known state/,
  );
  await actor.click(screen.getByText('View allocation chart data'));
  const table = await screen.findByRole('table', { name: 'Allocation chart data' });
  const last = within(table).getByText('Scope 8').closest('tr')!;
  expect(last).toHaveTextContent('Missing intervals confirmed');
  expect(last).toHaveTextContent('Projection from last known state: 2m (estimated)');
  await actor.click(screen.getByRole('button', { name: 'Allocation chart resource' }));
  await actor.click(screen.getByRole('menuitemradio', { name: 'Disk' }));
  expect(caption).toHaveTextContent('Missing intervals confirmed');
  expect(caption).toHaveTextContent('1h');
  expect(caption).toHaveTextContent('Projection from last known state: 2m (estimated)');
});

it('adds confirmed and projected durations within a Sandbox group without rounding large integers', async () => {
  const items = ['9007199254742398', '1'].map((unknown_seconds) => ({
    ...metricRow,
    allocated_period: {
      ...metricRow.allocated_period,
      unknown_seconds,
      projected_tail_seconds: '60',
    },
  }));
  render(
    <AllocationChart
      {...props}
      admin={false}
      page={{ report_id: props.reportId, as_of: asOf, next: null, total_count: 1, items }}
    />,
  );
  const caption = screen.getByLabelText('Plotted Sandboxes coverage');
  expect(caption).toHaveTextContent('104,249,991,374d 7h');
  expect(caption).toHaveTextContent('Projection from last known state: 2m (estimated)');
  await userEvent.click(screen.getByText('View allocation chart data'));
  const table = await screen.findByRole('table', { name: 'Allocation chart data' });
  expect(within(table).getAllByRole('row')).toHaveLength(2);
  expect(table).toHaveTextContent('104,249,991,374d 7h');
});

it('uses only owner evidence and the selected ranking, with unknown interval and projection details', async () => {
  const actor = userEvent.setup();
  const api = metricsClient(true);
  api.ownerAllocations.mockResolvedValue({
    report_id: props.reportId,
    metric: 'cpu',
    unit: 'core-hours',
    total_count: 6,
    next: null,
    items: Array.from({ length: 6 }, (_, index) => ({
      subject: `coverage-owner-${index}`,
      label: `Coverage owner ${index}`,
      allocated_hours: 6 - index,
      sandbox_count: 1,
      complete: index !== 5,
    })),
  });
  render(<OwnerAllocationChart {...props} api={api} metric="cpu" />);
  await screen.findByRole('region', { name: 'Scrollable owner allocations' });
  expect(screen.queryByText(/Partial history|Missing history is excluded/)).not.toBeInTheDocument();
  const caption = screen.getByLabelText('Selected owners coverage');
  expect(caption).toHaveTextContent('Coverage unverified');
  expect(caption).not.toHaveTextContent('No missing interval has been established');
  expect(caption).toHaveTextContent('Missing-interval durations are unavailable for this scope');
  expect(caption).toHaveTextContent('Projection details are unavailable for this scope');
  expect(caption).not.toHaveTextContent(/None confirmed|0s|estimated/);
  await actor.click(screen.getByText('View owner allocation chart data'));
  const table = await screen.findByRole('table', { name: 'Owner allocation chart data' });
  expect(within(table).getByText('Coverage owner 5').closest('tr')).toHaveTextContent(
    'Coverage unverified',
  );
  expect(within(table).getByText('Coverage owner 0').closest('tr')).toHaveTextContent(
    'Coverage verified',
  );
  await actor.click(screen.getByRole('button', { name: 'Owners shown' }));
  await actor.click(screen.getByRole('menuitemradio', { name: 'Top 5' }));
  expect(caption).toHaveTextContent('Coverage verified');
  expect(caption).not.toHaveTextContent(/Coverage unverified|Missing intervals confirmed/);
  expect(caption).toHaveTextContent('Projection details are unavailable for this scope');
  expect(api.create).not.toHaveBeenCalled();
});

it.each(['CPU', 'Disk'] as const)(
  'keeps the %s report gap visible when grouping allocations by owner',
  async (resource) => {
    const actor = userEvent.setup();
    const api = metricsClient(true);
    const coverage = {
      ...metricRow.allocated_period,
      complete: false,
      unknown_seconds: '3600',
      projected_tail_seconds: '0',
    };
    api.ownerAllocations.mockResolvedValue({
      report_id: props.reportId,
      metric: resource === 'CPU' ? 'cpu' : 'disk',
      unit: resource === 'CPU' ? 'core-hours' : 'GiB-hours',
      total_count: 1,
      next: null,
      items: [
        {
          subject: 'owner',
          label: 'Synthetic owner',
          allocated_hours: 10,
          sandbox_count: 1,
          complete: false,
        },
      ],
    });
    render(
      <AllocationChart
        {...props}
        api={api}
        admin
        reportCoverage={coverage}
        page={{
          report_id: props.reportId,
          as_of: asOf,
          next: null,
          total_count: 1,
          items: [{ ...metricRow, allocated_period: coverage }],
        }}
      />,
    );
    await actor.click(screen.getByRole('button', { name: 'Allocation chart resource' }));
    await actor.click(screen.getByRole('menuitemradio', { name: resource }));
    const report = screen.getByRole('note', { name: 'Report allocation coverage' });
    expect(report).toHaveTextContent('Missing intervals confirmed');
    expect(report).toHaveTextContent('Confirmed missing duration: 1h');
    await actor.click(screen.getByRole('button', { name: 'Allocation chart grouping' }));
    await actor.click(screen.getByRole('menuitemradio', { name: 'By owner' }));
    const owners = await screen.findByRole('note', { name: 'Selected owners coverage' });
    expect(owners).toHaveTextContent('Coverage unverified');
    expect(owners).toHaveTextContent('Missing-interval durations are unavailable for this scope');
    expect(owners).not.toHaveTextContent('No missing interval has been established');
    expect(report).toBeVisible();
    expect(report).toHaveTextContent('Confirmed missing duration: 1h');
  },
);
