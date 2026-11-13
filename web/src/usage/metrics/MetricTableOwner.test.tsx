import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { MetricTable } from './MetricTable';
import { metricsClient } from './testFixtures';

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
