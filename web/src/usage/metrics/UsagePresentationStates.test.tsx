import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { MetricsDashboard } from './MetricsDashboard';
import { defaultSelection } from './model';
import { metricsClient } from './testFixtures';

afterEach(() => vi.useRealTimers());

it('retries an unavailable report without a zero chart, then renders its allocated comparison', async () => {
  vi.useFakeTimers();
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
  await act(async () => {});
  expect(screen.getByText('Retrying usage automatically…')).toBeInTheDocument();
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(screen.queryByRole('img')).not.toBeInTheDocument();
  await act(async () => vi.advanceTimersByTimeAsync(1200));
  expect(screen.getByRole('region', { name: 'Allocation comparison chart' })).toBeInTheDocument();
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

it('keeps pending allocation unknown and disables refresh with bounded readable progress', () => {
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
  expect(view.container.querySelector('.usage-loading')).not.toBeInTheDocument();
  expect(screen.getByText(/Automatic checks pause after two minutes/)).toBeInTheDocument();
});
