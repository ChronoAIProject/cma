import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { ReportSelector } from './ReportSelector';
import { defaultSelection } from './model';
import { metricsClient } from './testFixtures';

it('searches all owner subjects and pages their frozen cohort without scanning Sandboxes', async () => {
  const actor = userEvent.setup();
  const api = metricsClient(true);
  const change = vi.fn();
  render(
    <ReportSelector
      api={api}
      selection={defaultSelection}
      kind="owner"
      value=""
      onChange={change}
      active
      admin
    />,
  );
  expect(api.create).not.toHaveBeenCalled();
  await actor.click(screen.getByRole('button', { name: 'Owner: Everyone' }));
  await screen.findByRole('button', { name: 'Choose owner' });
  expect(api.owners).toHaveBeenCalledWith(
    'report-one',
    expect.objectContaining({ limit: 50, search: '' }),
  );
  await actor.type(screen.getByRole('textbox', { name: 'Search owner choices' }), 'remote-owner');
  api.owners.mockResolvedValue({
    report_id: 'report-one',
    items: [{ subject: 'remote-owner-25001', display_name: 'Remote owner' }],
    next: 'next-owners',
  });
  await actor.click(screen.getByRole('button', { name: 'Search owner choices' }));
  await waitFor(() =>
    expect(api.owners).toHaveBeenLastCalledWith(
      'report-one',
      expect.objectContaining({ search: 'remote-owner', cursor: undefined, limit: 50 }),
    ),
  );
  await actor.click(screen.getByRole('button', { name: 'Next choices' }));
  await waitFor(() =>
    expect(api.owners).toHaveBeenLastCalledWith(
      'report-one',
      expect.objectContaining({ search: 'remote-owner', cursor: 'next-owners', limit: 50 }),
    ),
  );
  await actor.click(screen.getByRole('button', { name: 'Choose owner' }));
  await actor.click(
    screen.getByRole('menuitemradio', { name: 'Remote owner (remote-owner-25001)' }),
  );
  expect(change).toHaveBeenCalledWith('remote-owner-25001');
  expect(api.create).toHaveBeenCalledTimes(1);
  expect(api.sandboxes).not.toHaveBeenCalled();
});
it('distinguishes failed choices from empty choices and retries without changing the filter', async () => {
  const actor = userEvent.setup();
  const api = metricsClient(true);
  api.owners.mockRejectedValueOnce(new ApiError(503, 'Unavailable'));
  const change = vi.fn();
  render(
    <ReportSelector
      api={api}
      selection={defaultSelection}
      kind="owner"
      value="owner-1"
      onChange={change}
      active
      admin
    />,
  );
  await actor.click(screen.getByRole('button', { name: 'Owner: owner-1' }));
  await screen.findByRole('alert');
  expect(screen.queryByText('No matching owner choices.')).not.toBeInTheDocument();
  api.owners.mockResolvedValue({ report_id: 'report-one', items: [], next: null });
  await actor.click(screen.getByRole('button', { name: 'Retry usage' }));
  await screen.findByText('No matching owner choices.');
  expect(change).not.toHaveBeenCalled();
});
