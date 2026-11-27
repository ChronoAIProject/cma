import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { MetricTable } from './MetricTable';
import { metricRow, metricsClient } from './testFixtures';

it('keeps owner ranking and loaded totals while inventory pages and sorts change', async () => {
  const api = metricsClient(true);
  const props = {
    api,
    reportId: 'report-one',
    sort: 'name' as const,
    order: 'asc' as const,
    admin: true,
    active: true,
    measured: false,
    allocationMetric: 'cpu' as const,
    onAllocationMetricChange: vi.fn(),
    onSort: vi.fn(),
    onDetail: vi.fn(),
    onFailure: vi.fn(),
  };
  const view = render(<MetricTable {...props} />);
  const actor = userEvent.setup();
  await actor.click(screen.getByRole('button', { name: 'Allocation chart grouping' }));
  await actor.click(screen.getByRole('menuitemradio', { name: 'By owner' }));
  await screen.findByText(/1 of 1 owners · largest allocation first/);
  await actor.click(screen.getByRole('button', { name: 'Owners shown' }));
  await actor.click(screen.getByRole('menuitemradio', { name: 'All' }));
  await actor.click(screen.getByRole('button', { name: 'Next usage page' }));
  await waitFor(() =>
    expect(api.sandboxes).toHaveBeenLastCalledWith(
      'report-one',
      expect.objectContaining({ cursor: 'page-two' }),
    ),
  );
  view.rerender(<MetricTable {...props} sort="allocated_cpu_core_hours" order="desc" />);
  await waitFor(() =>
    expect(api.sandboxes).toHaveBeenLastCalledWith(
      'report-one',
      expect.objectContaining({ cursor: undefined, sort: 'allocated_cpu_core_hours' }),
    ),
  );
  view.rerender(<MetricTable {...props} />);
  await waitFor(() =>
    expect(api.sandboxes).toHaveBeenLastCalledWith(
      'report-one',
      expect.objectContaining({ cursor: undefined, sort: 'name' }),
    ),
  );
  expect(screen.getByRole('button', { name: 'Owners shown' })).toHaveTextContent('All');
  expect(screen.getByText(/1 of 1 owners loaded.*All owners loaded/)).toBeVisible();
  expect(api.ownerAllocations).toHaveBeenCalledTimes(1);
  expect(api.create).not.toHaveBeenCalled();
});

it.each([
  'allocated_cpu_core_hours',
  'allocated_memory_gib_hours',
  'allocated_disk_gib_hours',
] as const)('preserves API page order and resets cursors for %s', async (sort) => {
  const api = metricsClient();
  const supplied = [47.935, 8.165, 0].map((value, index) => ({
    ...metricRow,
    sandbox_id: `sandbox-${index}`,
    name: `Subtotal ${index}`,
    allocated_period: {
      ...metricRow.allocated_period,
      cpu_core_hours: value,
      memory_gib_hours: value,
      disk_gib_hours: value,
      complete: index === 1,
      unknown_seconds: index === 1 ? 0 : 3600,
    },
  }));
  api.sandboxes.mockResolvedValue({
    report_id: 'report-one',
    as_of: 1,
    items: supplied,
    next: 'page-two',
    total_count: 61,
  });
  const props = {
    api,
    reportId: 'report-one',
    sort,
    order: 'asc' as const,
    admin: false,
    active: true,
    measured: false,
    allocationMetric: 'cpu' as const,
    onAllocationMetricChange: vi.fn(),
    onSort: vi.fn(),
    onDetail: vi.fn(),
    onFailure: vi.fn(),
  };
  const view = render(<MetricTable key={props.reportId} {...props} />);
  const actor = userEvent.setup();
  const table = await screen.findByRole('table', { name: 'Sandboxes and allocated usage' });
  await within(table).findByRole('button', { name: 'Subtotal 0' });
  expect(
    within(table)
      .getAllByRole('button', { name: /Subtotal/ })
      .map((b) => b.textContent),
  ).toEqual(supplied.map((row) => row.name));
  expect(within(table).getAllByText('Missing intervals confirmed')).toHaveLength(2);
  await actor.click(screen.getByRole('button', { name: 'Next usage page' }));
  await waitFor(() =>
    expect(api.sandboxes).toHaveBeenLastCalledWith(
      'report-one',
      expect.objectContaining({ sort, order: 'asc', cursor: 'page-two', limit: 50 }),
    ),
  );
  view.rerender(<MetricTable key={props.reportId} {...props} order="desc" />);
  await waitFor(() =>
    expect(api.sandboxes).toHaveBeenLastCalledWith(
      'report-one',
      expect.objectContaining({ sort, order: 'desc', cursor: undefined }),
    ),
  );
  await actor.click(screen.getByRole('button', { name: 'Next usage page' }));
  await waitFor(() =>
    expect(api.sandboxes).toHaveBeenLastCalledWith(
      'report-one',
      expect.objectContaining({ cursor: 'page-two' }),
    ),
  );
  view.rerender(<MetricTable key="replacement" {...props} reportId="replacement" order="desc" />);
  await waitFor(() =>
    expect(api.sandboxes).toHaveBeenLastCalledWith(
      'replacement',
      expect.objectContaining({ sort, order: 'desc', cursor: undefined }),
    ),
  );
  expect(screen.getByRole('button', { name: 'Previous usage page' })).toBeDisabled();
  expect(api.create).not.toHaveBeenCalled();
});
