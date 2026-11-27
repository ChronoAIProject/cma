import { act, render, screen } from '@testing-library/react';
import type { PauseIdleNow } from '@cma/client';
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

it('shows each Sandbox with its own preview refusal', async () => {
  const client = operatorClient();
  client.admin.previewAutoPause.mockResolvedValueOnce({
    candidates: 0,
    refusals: [
      { sandbox_id: 'sbx_legacy', reason: 'legacy_log_expected_rows' },
      { sandbox_id: 'sbx_recent', reason: 'recent_agent_activity' },
    ],
  });
  const user = userEvent.setup();
  render(<AutoPauseControls client={client} disabled={false} />);
  await user.click(screen.getByRole('button', { name: 'Preview idle Sandboxes' }));
  await screen.findByRole('list', { name: 'Sandbox preview refusals' });
  const [legacy, recent] = screen.getAllByRole('listitem');
  expect(legacy).toHaveTextContent('sbx_legacy · legacy log expected rows');
  expect(recent).toHaveTextContent('sbx_recent · recent agent activity');
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

it('uses the saved policy result when auto pause is disabled after preview', async () => {
  const client = operatorClient();
  client.admin.previewAutoPause.mockResolvedValueOnce({ candidates: 2, enabled: true });
  client.admin.pauseIdleNow.mockResolvedValueOnce({
    enabled: false,
    scheduled: 0,
    already_satisfied: 0,
    skipped: 0,
    failed: 0,
    next: null,
  });
  const user = userEvent.setup();
  render(<AutoPauseControls client={client} disabled={false} />);
  await user.click(screen.getByRole('button', { name: 'Preview idle Sandboxes' }));
  await screen.findByText('2 eligible Sandboxes in this preview batch.');
  await user.click(screen.getByRole('button', { name: 'Pause idle now' }));
  expect(
    screen.getByText(/Active work, the idle threshold, and Always on exceptions are rechecked/),
  ).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Confirm pause batch' }));
  await screen.findByText('Auto pause is disabled; no pauses were scheduled.');
  expect(client.admin.pauseIdleNow).toHaveBeenCalledExactlyOnceWith(true, {
    limit: 25,
    cursor: undefined,
  });
  expect(screen.queryByRole('button', { name: 'Confirm pause batch' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Pause next batch' })).not.toBeInTheDocument();
});

it('cancels without a write and prevents dispatch when controls become unavailable', async () => {
  const client = operatorClient();
  const user = userEvent.setup();
  const view = render(<AutoPauseControls client={client} disabled={false} />);
  await user.click(screen.getByRole('button', { name: 'Pause idle now' }));
  await user.click(screen.getByRole('button', { name: 'Cancel' }));
  expect(screen.queryByRole('button', { name: 'Confirm pause batch' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Pause idle now' }));
  view.rerender(<AutoPauseControls client={client} disabled />);
  for (const name of ['Preview idle Sandboxes', 'Pause idle now', 'Confirm pause batch']) {
    const button = screen.getByRole('button', { name });
    expect(button).toBeDisabled();
    await user.click(button);
  }
  expect(client.admin.pauseIdleNow).not.toHaveBeenCalled();
  expect(client.admin.previewAutoPause).not.toHaveBeenCalled();
});

it('dispatches one confirmed batch while its outcome is pending', async () => {
  const client = operatorClient();
  let resolve!: (result: PauseIdleNow) => void;
  client.admin.pauseIdleNow.mockReturnValueOnce(
    new Promise((complete) => {
      resolve = complete;
    }),
  );
  const user = userEvent.setup();
  render(<AutoPauseControls client={client} disabled={false} />);
  await user.click(screen.getByRole('button', { name: 'Pause idle now' }));
  await user.dblClick(screen.getByRole('button', { name: 'Confirm pause batch' }));
  expect(client.admin.pauseIdleNow).toHaveBeenCalledTimes(1);
  expect(screen.getByRole('status')).toHaveTextContent('Checking this batch');
  expect(screen.getByRole('button', { name: 'Preview idle Sandboxes' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Cancel' })).toBeDisabled();
  await act(async () =>
    resolve({ scheduled: 1, already_satisfied: 1, skipped: 1, failed: 0, next: null }),
  );
  expect(
    screen.getByText('1 scheduled, 1 already scheduled, 1 skipped in this batch.'),
  ).toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Pause idle now' })).toBeEnabled();
  expect(client.admin.pauseIdleNow).toHaveBeenCalledTimes(1);
});
