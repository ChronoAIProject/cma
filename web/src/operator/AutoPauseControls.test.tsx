import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { AutoPauseControls } from './AutoPauseControls';
import { operatorClient } from './testFixtures';

it('previews bounded pages and keeps prior counts when a read fails', async () => {
  const client = operatorClient();
  client.admin.previewAutoPause.mockResolvedValueOnce({
    candidates: 2,
    next: 'sbx_next',
    failed: 0,
  });
  client.admin.previewAutoPause.mockRejectedValueOnce(new Error('offline'));
  const user = userEvent.setup();
  render(<AutoPauseControls client={client} disabled={false} />);
  await user.click(screen.getByRole('button', { name: 'Preview idle Sandboxes' }));
  await screen.findByText('2 eligible Sandboxes in this preview batch.');
  await user.click(screen.getByRole('button', { name: 'Preview next batch' }));
  await screen.findByText(/operation could not be confirmed/);
  expect(screen.getByText('2 eligible Sandboxes in this preview batch.')).toBeInTheDocument();
  expect(client.admin.previewAutoPause).toHaveBeenLastCalledWith({ limit: 25, cursor: 'sbx_next' });
  expect(client.admin.pauseIdleNow).not.toHaveBeenCalled();
});

it('requires confirmation for each batch and retains an uncertain first-candidate cursor', async () => {
  const client = operatorClient();
  client.admin.pauseIdleNow.mockResolvedValueOnce({
    scheduled: 0,
    already_satisfied: 0,
    skipped: 0,
    failed: 1,
    next: '',
  });
  client.admin.pauseIdleNow.mockResolvedValueOnce({
    scheduled: 1,
    already_satisfied: 0,
    skipped: 0,
    failed: 0,
    next: null,
  });
  const user = userEvent.setup();
  render(<AutoPauseControls client={client} disabled={false} />);
  await user.click(screen.getByRole('button', { name: 'Pause idle now' }));
  expect(client.admin.pauseIdleNow).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Confirm pause batch' }));
  await screen.findByText(/Some outcomes could not be confirmed/);
  expect(client.admin.pauseIdleNow).toHaveBeenCalledTimes(1);
  await user.click(screen.getByRole('button', { name: 'Review unconfirmed batch' }));
  await user.click(screen.getByRole('button', { name: 'Confirm pause batch' }));
  await screen.findByText('1 scheduled, 0 already scheduled, 0 skipped in this batch.');
  expect(client.admin.pauseIdleNow).toHaveBeenLastCalledWith(true, { limit: 25, cursor: '' });
  expect(screen.queryByRole('button', { name: 'Pause next batch' })).not.toBeInTheDocument();
});

it('reports disabled policy and retains confirmation after a rejected write', async () => {
  const client = operatorClient();
  client.admin.previewAutoPause.mockResolvedValueOnce({ candidates: 0, enabled: false });
  client.admin.pauseIdleNow.mockRejectedValueOnce(new Error('offline'));
  const user = userEvent.setup();
  render(<AutoPauseControls client={client} disabled={false} />);
  await user.click(screen.getByRole('button', { name: 'Preview idle Sandboxes' }));
  await screen.findByText('Auto pause is disabled in saved settings.');
  await user.click(screen.getByRole('button', { name: 'Pause idle now' }));
  await user.click(screen.getByRole('button', { name: 'Confirm pause batch' }));
  await screen.findByText(/operation could not be confirmed/);
  expect(screen.getByRole('button', { name: 'Confirm pause batch' })).toBeEnabled();
  expect(client.admin.pauseIdleNow).toHaveBeenCalledTimes(1);
});
