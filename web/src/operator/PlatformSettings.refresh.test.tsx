import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { PlatformSettings } from './PlatformSettings';
import { operatorClient, settings, stored } from './testFixtures';

it('refreshes clean settings after edited numeric values are restored', async () => {
  const client = operatorClient();
  render(<PlatformSettings client={client} />);
  for (const label of [
    'Agent log retention (days)',
    'Sandboxes per user',
    'Visitor Sandboxes',
    'Visitor launches per day',
  ]) {
    const input = (await screen.findByLabelText(label)) as HTMLInputElement;
    const original = input.value;
    fireEvent.change(input, { target: { value: '-1' } });
    fireEvent.change(input, { target: { value: original } });
  }
  await userEvent.click(screen.getByRole('button', { name: 'Refresh settings availability' }));
  await waitFor(() => expect(client.admin.readSettings).toHaveBeenCalledTimes(2));
  expect(screen.queryByText(/Your draft is retained/)).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Save settings' })).toBeEnabled();
  expect(client.admin.replaceSettings).not.toHaveBeenCalled();
});

it('loads a newer revision on a clean refresh and preserves compatibility fields', async () => {
  const client = operatorClient();
  const latest = {
    ...stored,
    revision: 9,
    settings: { ...settings, future: 'newer', retention: { agent_log_days: 45 } },
  };
  client.admin.readSettings.mockResolvedValueOnce(stored).mockResolvedValue(latest);
  render(<PlatformSettings client={client} />);
  await userEvent.click(
    await screen.findByRole('button', { name: 'Refresh settings availability' }),
  );
  await screen.findByText('Revision 9');
  expect(screen.getByLabelText('Agent log retention (days)')).toHaveValue(45);
  await userEvent.click(screen.getByRole('button', { name: 'Save settings' }));
  await waitFor(() =>
    expect(client.admin.replaceSettings).toHaveBeenCalledWith({
      expected_revision: 9,
      settings: latest.settings,
    }),
  );
});

it('retains actual edits across a refresh and allows explicit review', async () => {
  const client = operatorClient();
  render(<PlatformSettings client={client} />);
  const input = await screen.findByLabelText('Sandboxes per user');
  fireEvent.change(input, { target: { value: '11' } });
  await userEvent.click(screen.getByRole('button', { name: 'Refresh settings availability' }));
  await screen.findByText(/Your draft is retained/);
  expect(input).toHaveValue(11);
  expect(screen.getByRole('button', { name: 'Save settings' })).toBeDisabled();
  expect(client.admin.replaceSettings).not.toHaveBeenCalled();
});

it('keeps the draft after refresh failure without sending a write', async () => {
  const client = operatorClient();
  client.admin.readSettings
    .mockResolvedValueOnce(stored)
    .mockRejectedValue(new Error('Unavailable'));
  render(<PlatformSettings client={client} />);
  const input = await screen.findByLabelText('Sandboxes per user');
  fireEvent.change(input, { target: { value: '11' } });
  await userEvent.click(screen.getByRole('button', { name: 'Refresh settings availability' }));
  await screen.findByRole('alert');
  expect(input).toHaveValue(11);
  expect(client.admin.replaceSettings).not.toHaveBeenCalled();
});

it('prevents edits and overlapping writes while a clean refresh is pending', async () => {
  const client = operatorClient();
  let finish!: (value: typeof stored) => void;
  client.admin.readSettings.mockResolvedValueOnce(stored).mockImplementation(
    () =>
      new Promise((resolve) => {
        finish = resolve;
      }),
  );
  render(<PlatformSettings client={client} />);
  const refresh = await screen.findByRole('button', { name: 'Refresh settings availability' });
  await userEvent.dblClick(refresh);
  expect(client.admin.readSettings).toHaveBeenCalledTimes(2);
  expect(refresh).toBeDisabled();
  expect(screen.getByLabelText('Sandboxes per user')).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Refreshing' })).toBeDisabled();
  await act(async () => finish({ ...stored, revision: 8 }));
  expect(screen.getByText('Revision 8')).toBeInTheDocument();
  expect(client.admin.replaceSettings).not.toHaveBeenCalled();
});

it('clears draft review after all edits are restored without clearing write recovery', async () => {
  const client = operatorClient();
  render(<PlatformSettings client={client} />);
  const input = await screen.findByLabelText('Sandboxes per user');
  fireEvent.change(input, { target: { value: '11' } });
  await userEvent.click(screen.getByRole('button', { name: 'Refresh settings availability' }));
  await screen.findByText(/Your draft is retained/);
  fireEvent.change(input, { target: { value: '10' } });
  await userEvent.click(screen.getByRole('button', { name: 'Refresh settings availability' }));
  await waitFor(() => expect(screen.queryByText(/Your draft is retained/)).not.toBeInTheDocument());
  expect(client.admin.replaceSettings).not.toHaveBeenCalled();
});

it('keeps an uncertain write in review across repeated clean refreshes', async () => {
  const client = operatorClient();
  client.admin.replaceSettings.mockRejectedValue(new Error('Disconnected'));
  render(<PlatformSettings client={client} />);
  await userEvent.click(await screen.findByRole('button', { name: 'Save settings' }));
  await screen.findByText(/Your draft is retained/);
  for (let attempt = 0; attempt < 2; attempt += 1) {
    await userEvent.click(screen.getByRole('button', { name: 'Refresh settings availability' }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Refresh settings availability' })).toBeEnabled(),
    );
    expect(screen.getByText(/Your draft is retained/)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save settings' })).toBeDisabled();
  }
  expect(client.admin.replaceSettings).toHaveBeenCalledOnce();
});
