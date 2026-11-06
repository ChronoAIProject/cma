import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { OwnerAllocationChart } from './OwnerAllocationChart';
import { OwnerAllocationName } from './OwnerAllocationName';
import { metricsClient } from './testFixtures';

it('retains cohort disambiguators across the 200/201 page boundary in labels, tooltips and table', async () => {
  const api = metricsClient(true);
  const items = Array.from({ length: 201 }, (_, index) => ({
    subject: `private-subject-${index}`,
    label: index >= 199 ? 'Alex Chen' : `Owner ${index}`,
    ...(index >= 199 ? { disambiguator: `Joined 2026-0${index - 198}-01` } : {}),
    allocated_hours: 1,
    sandbox_count: 1,
    complete: true,
  }));
  api.ownerAllocations.mockImplementation((id, query) => {
    const offset = Number(query.cursor ?? 0);
    return Promise.resolve({
      report_id: id,
      metric: 'cpu',
      unit: 'core-hours',
      items: items.slice(offset, offset + 200),
      total_count: 201,
      next: offset ? null : '200',
    });
  });
  render(
    <OwnerAllocationChart
      api={api}
      reportId="report-one"
      metric="cpu"
      active
      onFailure={vi.fn()}
    />,
  );
  await screen.findByText(/10 of 201 owners/);
  await userEvent.click(screen.getByRole('button', { name: 'Owners shown' }));
  await userEvent.click(screen.getByRole('menuitemradio', { name: 'All', exact: true }));
  await userEvent.click(screen.getByRole('button', { name: 'Load more owners' }));
  await screen.findByText(/201 of 201 owners loaded/);
  const scroll = screen.getByRole('region', { name: 'Scrollable owner allocations' });
  fireEvent.scroll(scroll, { target: { scrollTop: 199 * 48 } });
  for (const date of ['2026-01-01', '2026-02-01']) {
    const name = within(scroll).getByTitle(`Alex Chen · Joined ${date}`);
    fireEvent.focus(name);
    expect(screen.getByRole('tooltip')).toHaveTextContent(`Alex Chen · Joined ${date}`);
    fireEvent.blur(name);
  }
  await userEvent.click(screen.getByText('View owner allocation chart data'));
  for (let index = 0; index < 9; index++)
    await userEvent.click(screen.getByRole('button', { name: 'Next owner values' }));
  const table = screen.getByRole('table', { name: 'Owner allocation chart data' });
  expect(within(table).getByTitle('Alex Chen · Joined 2026-01-01')).toBeVisible();
  await userEvent.click(screen.getByRole('button', { name: 'Next owner values' }));
  expect(within(table).getByTitle('Alex Chen · Joined 2026-02-01')).toBeVisible();
  expect(document.body.innerHTML).not.toContain('private-subject');
  expect(api.create).not.toHaveBeenCalled();
});

it('portals full-name disclosure out of the clipping viewport and dismisses it with Escape', async () => {
  render(
    <div style={{ overflow: 'hidden', height: 48 }}>
      <OwnerAllocationName
        label="Alexandra Montgomery Research Operations"
        disambiguator="Joined 2026-01-01"
      />
    </div>,
  );
  const anchor = screen.getByTitle('Alexandra Montgomery Research Operations · Joined 2026-01-01');
  await userEvent.tab();
  expect(anchor).toHaveFocus();
  const tooltip = screen.getByRole('tooltip');
  expect(tooltip.parentElement).toBe(document.body);
  expect(anchor).toHaveAttribute('aria-describedby', tooltip.id);
  await userEvent.keyboard('{Escape}');
  await waitFor(() => expect(screen.queryByRole('tooltip')).not.toBeInTheDocument());
  expect(anchor).toHaveFocus();
});
