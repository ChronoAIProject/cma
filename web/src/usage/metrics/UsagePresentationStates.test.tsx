import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { MetricsDashboard } from './MetricsDashboard';
import { defaultSelection } from './model';
import { metricsClient } from './testFixtures';

it('retries an unavailable report without a zero chart, then renders its allocated comparison', async () => {
  const actor = userEvent.setup();
  const api = metricsClient();
  api.create.mockRejectedValueOnce(new ApiError(503, 'Fixture unavailable'));
  render(
    <MetricsDashboard
      api={api}
      history={{ events: vi.fn() }}
      admin={false}
      selection={defaultSelection}
      onSelection={vi.fn()}
    />,
  );
  expect(await screen.findByRole('alert')).toHaveTextContent('unavailable data is not zero usage');
  expect(screen.queryByRole('img')).not.toBeInTheDocument();
  const retry = screen.getByRole('button', { name: 'Retry usage' });
  expect(retry).toHaveClass('button--outline');
  await actor.click(retry);
  expect(
    await screen.findByRole('region', { name: 'Allocation comparison chart' }),
  ).toBeInTheDocument();
  expect(api.series).not.toHaveBeenCalled();
});

it('preserves key server sorts in the compact table and hides all measured sort choices', async () => {
  const actor = userEvent.setup();
  const onSelection = vi.fn();
  render(
    <MetricsDashboard
      api={metricsClient(true)}
      history={{ events: vi.fn() }}
      admin
      selection={defaultSelection}
      onSelection={onSelection}
    />,
  );
  await actor.click(
    await screen.findByRole('button', { name: 'Sort by CPU (core-hours) ascending' }),
  );
  expect(onSelection).toHaveBeenLastCalledWith(
    expect.objectContaining({ sort: 'allocated_cpu_core_hours', order: 'asc' }),
  );
  await actor.click(screen.getByRole('button', { name: 'Sort usage inventory' }));
  expect(screen.queryByRole('menuitemradio', { name: /Measured/ })).not.toBeInTheDocument();
  await actor.click(screen.getByRole('menuitemradio', { name: 'Status' }));
  expect(onSelection).toHaveBeenLastCalledWith(expect.objectContaining({ sort: 'status' }));
});

it('keeps pending allocation unknown and disables refresh while reserving the report layout', () => {
  const api = metricsClient();
  api.create.mockReturnValue(new Promise(() => {}));
  const view = render(
    <MetricsDashboard
      api={api}
      history={{ events: vi.fn() }}
      admin={false}
      selection={defaultSelection}
      onSelection={vi.fn()}
    />,
  );
  expect(screen.getByRole('status')).toHaveTextContent('Loading usage report');
  expect(screen.getByRole('button', { name: 'Refresh usage' })).toBeDisabled();
  expect(
    screen.queryByRole('region', { name: 'Allocated resource-hours' }),
  ).not.toBeInTheDocument();
  expect(screen.queryByRole('img')).not.toBeInTheDocument();
  expect(view.container.querySelector('.usage-loading')).toBeInTheDocument();
});
