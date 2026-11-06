import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ApiError } from '@chronoai/cma-sdk';
import { expect, it, vi } from 'vitest';
import { OwnerAllocationChart } from './OwnerAllocationChart';
import { metricsClient } from './testFixtures';
import type { Metric } from './model';

const owners = (count: number) =>
  Array.from({ length: count }, (_, index) => ({
    subject: `91892afa-5fa5-4f4f-9100-${String(index).padStart(12, '0')}`,
    label:
      index < 2
        ? 'Alex Chen'
        : index === 2
          ? 'Unnamed owner'
          : `Owner ${String(index).padStart(5, '0')}`,
    ...(index < 2 ? { disambiguator: `Joined 2026-0${index + 1}-01` } : {}),
    allocated_hours: count - index,
    sandbox_count: 2,
    complete: index !== 0,
  }));
function fixture(count = 400, pageSize = 200) {
  const api = metricsClient(true);
  const rows = owners(count);
  api.ownerAllocations.mockImplementation(
    (id: string, options: { metric: Metric; cursor?: string }) => {
      const offset = Number(options.cursor ?? 0);
      return Promise.resolve({
        report_id: id,
        metric: options.metric,
        unit: options.metric === 'cpu' ? 'core-hours' : 'GiB-hours',
        items: rows.slice(offset, offset + pageSize),
        next: offset + pageSize < count ? String(offset + pageSize) : null,
        total_count: count,
      });
    },
  );
  return {
    api,
    props: {
      api,
      reportId: 'report-one',
      metric: 'cpu' as const,
      active: true,
      onFailure: vi.fn(),
    },
  };
}
async function select(value: string) {
  const actor = userEvent.setup();
  await actor.click(screen.getByRole('button', { name: 'Owners shown' }));
  await actor.click(screen.getByRole('menuitemradio', { name: value }));
}

it('defaults to Top 10, changes only the displayed ranking and never creates a report', async () => {
  const { api, props } = fixture();
  render(<OwnerAllocationChart {...props} />);
  expect(await screen.findByText('10 of 400 owners · largest allocation first')).toBeVisible();
  await select('Top 5');
  expect(screen.getByText('5 of 400 owners · largest allocation first')).toBeVisible();
  expect(screen.getAllByRole('listitem')).toHaveLength(5);
  await select('Top 50');
  expect(screen.getByText('50 of 400 owners · largest allocation first')).toBeVisible();
  expect(screen.getAllByRole('listitem').length).toBeLessThanOrEqual(12);
  await select('All');
  expect(screen.getByText(/200 of 400 owners loaded/)).toBeVisible();
  expect(api.ownerAllocations).toHaveBeenCalledTimes(1);
  expect(api.create).not.toHaveBeenCalled();
  await userEvent.click(screen.getByText('View owner allocation chart data'));
  const table = screen.getByRole('table', { name: 'Owner allocation chart data' });
  expect(within(table).getAllByRole('row')).toHaveLength(21);
  await userEvent.click(screen.getByRole('button', { name: 'Next owner values' }));
  expect(within(table).getByTitle('Owner 00020')).toBeInTheDocument();
  expect(table.innerHTML).not.toContain('91892afa');
  await userEvent.click(screen.getByRole('button', { name: 'Load more owners' }));
  expect(await screen.findByText(/400 of 400 owners loaded.*All owners loaded/)).toBeVisible();
  expect(api.ownerAllocations).toHaveBeenCalledTimes(2);
  await select('Top 10');
  expect(api.ownerAllocations).toHaveBeenCalledTimes(2);
});

it('keeps 20,001 loaded owners bounded, scrolls aligned rows, exposes names and never opaque IDs', async () => {
  const { props } = fixture(20001, 20001);
  const { container } = render(<OwnerAllocationChart {...props} />);
  await screen.findByText(/10 of 20,001 owners/);
  await select('All');
  const scroll = screen.getByRole('region', { name: 'Scrollable owner allocations' });
  expect(scroll).toHaveAttribute('tabindex', '0');
  expect(screen.getAllByRole('listitem').length).toBeLessThanOrEqual(12);
  expect(screen.getAllByTitle('Alex Chen')).toHaveLength(2);
  expect(screen.getByText('Joined 2026-01-01')).toBeVisible();
  expect(container.innerHTML).not.toContain('91892afa');
  fireEvent.scroll(scroll, { target: { scrollTop: 48 * 10000 } });
  expect(screen.getAllByRole('listitem')[0]).toHaveAttribute('aria-posinset', '9999');
  expect(screen.getAllByRole('listitem')[0]).toHaveAttribute('aria-setsize', '20001');
  expect(screen.getAllByRole('listitem').length).toBeLessThanOrEqual(12);
  expect(within(scroll).getByTitle('Owner 10000')).toBeInTheDocument();
});

it('shows loading, empty, failure and retry without treating a failure as zero allocation', async () => {
  const { api, props } = fixture(0);
  api.ownerAllocations.mockRejectedValueOnce(new ApiError(503, 'unavailable'));
  render(<OwnerAllocationChart {...props} />);
  expect(screen.getByText('Loading owner allocations…')).toBeVisible();
  expect(await screen.findByRole('alert')).toHaveTextContent('Usage reporting is unavailable');
  expect(screen.queryByText('No owners match this selection.')).not.toBeInTheDocument();
  await userEvent.click(screen.getByRole('button', { name: 'Retry usage' }));
  expect(await screen.findByText('No owners match this selection.')).toBeVisible();
  expect(screen.queryByRole('region')).not.toBeInTheDocument();
});

it('honors reporting backoff without issuing automatic retries or changing report filters', async () => {
  const { api, props } = fixture();
  api.ownerAllocations.mockRejectedValueOnce(
    new ApiError(429, 'busy', undefined, undefined, 60000),
  );
  render(<OwnerAllocationChart {...props} />);
  expect(await screen.findByRole('alert')).toHaveTextContent('Usage reporting is busy');
  const retry = screen.getByRole('button', { name: 'Waiting to retry' });
  expect(retry).toBeDisabled();
  await userEvent.click(retry);
  await select('All');
  expect(api.ownerAllocations).toHaveBeenCalledTimes(1);
  expect(api.create).not.toHaveBeenCalled();
});

it('preserves a partial page on continuation failure, and clears data after access expires', async () => {
  const { api, props } = fixture();
  render(<OwnerAllocationChart {...props} />);
  await screen.findByText(/10 of 400/);
  await select('All');
  api.ownerAllocations.mockRejectedValueOnce(new ApiError(503, 'unavailable'));
  await userEvent.click(screen.getByRole('button', { name: 'Load more owners' }));
  expect(await screen.findByRole('alert')).toBeVisible();
  expect(screen.getByText(/200 of 400 owners loaded/)).toBeVisible();
  api.ownerAllocations.mockRejectedValueOnce(new ApiError(410, 'expired'));
  await userEvent.click(screen.getByRole('button', { name: 'Retry usage' }));
  await waitFor(() =>
    expect(props.onFailure).toHaveBeenCalledWith(expect.objectContaining({ expired: true })),
  );
  expect(screen.queryByRole('listitem')).not.toBeInTheDocument();
});

it('cancels stale resource reads, resumes after being hidden and reuses loaded resource pages', async () => {
  const { api, props } = fixture();
  let resolve!: (value: unknown) => void;
  api.ownerAllocations.mockImplementationOnce(
    () =>
      new Promise((done) => {
        resolve = done;
      }),
  );
  const view = render(<OwnerAllocationChart {...props} />);
  const signal = api.ownerAllocations.mock.calls[0]?.[1].signal;
  view.rerender(<OwnerAllocationChart {...props} metric="memory" />);
  expect(signal?.aborted).toBe(true);
  await screen.findByText(/10 of 400/);
  resolve({ items: owners(1), total_count: 1, next: null });
  expect(screen.queryByText(/1 of 1 owners/)).not.toBeInTheDocument();
  view.rerender(<OwnerAllocationChart {...props} active={false} />);
  expect(api.ownerAllocations).toHaveBeenCalledTimes(2);
  view.rerender(<OwnerAllocationChart {...props} />);
  await waitFor(() => expect(api.ownerAllocations).toHaveBeenCalledTimes(3));
  view.rerender(<OwnerAllocationChart {...props} metric="memory" />);
  expect(api.ownerAllocations).toHaveBeenCalledTimes(3);
});
