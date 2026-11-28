import { openSettingsEditors } from './settingsTestSupport';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { PlatformSettings } from './PlatformSettings';
import { operatorClient, settings, stored } from './testFixtures';
import { reviewSettingsDraft } from './settingsDraft';

it('defaults Show measured usage off for old settings and only notifies after a successful save', async () => {
  const client = operatorClient();
  const actor = userEvent.setup();
  let save!: (value: typeof stored) => void;
  client.admin.replaceSettings.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        save = resolve;
      }),
  );
  const changed = vi.fn();
  window.addEventListener('cma:usage-settings', changed);
  try {
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    const toggle = await screen.findByRole('checkbox', { name: 'Show measured usage' });
    expect(toggle).not.toBeChecked();
    await actor.click(toggle);
    expect(client.admin.replaceSettings).not.toHaveBeenCalled();
    expect(changed).not.toHaveBeenCalled();
    await actor.click(screen.getByRole('button', { name: 'Save settings' }));
    expect(toggle).toBeDisabled();
    expect(changed).not.toHaveBeenCalled();
    const next = { ...settings, usage_reporting: { measured_visible: true } };
    expect(client.admin.replaceSettings).toHaveBeenCalledWith({
      expected_revision: 4,
      settings: next,
    });
    await act(async () => save({ settings: next, revision: 5 }));
    expect(changed).toHaveBeenCalledTimes(1);
    expect(toggle).toBeChecked();
    await screen.findByText('Settings saved at revision 5.');
  } finally {
    window.removeEventListener('cma:usage-settings', changed);
  }
});
it('allows an administrator to turn measured visibility off', async () => {
  const client = operatorClient();
  client.admin.readSettings.mockResolvedValue({
    ...stored,
    settings: { ...settings, usage_reporting: { measured_visible: true } },
  });
  const actor = userEvent.setup();
  render(<PlatformSettings client={client} />);
  await openSettingsEditors();
  const toggle = await screen.findByRole('checkbox', { name: 'Show measured usage' });
  expect(toggle).toBeChecked();
  await actor.click(toggle);
  await actor.click(screen.getByRole('button', { name: 'Save settings' }));
  await screen.findByText('Settings saved at revision 5.');
  expect(client.admin.replaceSettings).toHaveBeenCalledWith({
    expected_revision: 4,
    settings: { ...settings, usage_reporting: { measured_visible: false } },
  });
});
it('retains a conflicted visibility draft and requires review before another write', async () => {
  const client = operatorClient();
  const actor = userEvent.setup();
  client.admin.readSettings.mockResolvedValueOnce(stored).mockResolvedValue({
    revision: 8,
    settings: {
      ...settings,
      retention: { agent_log_days: 45 },
      usage_reporting: { measured_visible: false },
    },
  });
  client.admin.replaceSettings.mockRejectedValueOnce(
    new ApiError(409, 'Conflict', 'settings_conflict'),
  );
  const changed = vi.fn();
  window.addEventListener('cma:usage-settings', changed);
  try {
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    await actor.click(await screen.findByRole('checkbox', { name: 'Show measured usage' }));
    await actor.click(screen.getByRole('button', { name: 'Save settings' }));
    await screen.findByText(/The current revision is 8/);
    expect(screen.getByRole('button', { name: 'Save settings' })).toBeDisabled();
    expect(changed).not.toHaveBeenCalled();
    await actor.click(screen.getByRole('button', { name: 'Review retained draft' }));
    expect(screen.getByRole('checkbox', { name: 'Show measured usage' })).toBeChecked();
    expect(client.admin.replaceSettings).toHaveBeenCalledTimes(1);
    await actor.click(screen.getByRole('button', { name: 'Save settings' }));
    await screen.findByText('Settings saved at revision 9.');
    expect(client.admin.replaceSettings).toHaveBeenLastCalledWith({
      expected_revision: 8,
      settings: {
        ...settings,
        retention: { agent_log_days: 45 },
        usage_reporting: { measured_visible: true },
      },
    });
    expect(changed).toHaveBeenCalledTimes(1);
  } finally {
    window.removeEventListener('cma:usage-settings', changed);
  }
});
it('does not announce a refused visibility write as saved', async () => {
  const client = operatorClient();
  client.admin.replaceSettings.mockRejectedValue(new ApiError(403, 'Forbidden'));
  const actor = userEvent.setup();
  const changed = vi.fn();
  window.addEventListener('cma:usage-settings', changed);
  try {
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    await actor.click(await screen.findByRole('checkbox', { name: 'Show measured usage' }));
    await actor.click(screen.getByRole('button', { name: 'Save settings' }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Save settings' })).toBeEnabled(),
    );
    expect(screen.queryByText(/Settings saved/)).not.toBeInTheDocument();
    expect(changed).not.toHaveBeenCalled();
  } finally {
    window.removeEventListener('cma:usage-settings', changed);
  }
});
it('retains only an explicitly edited visibility value while preserving newer server fields', () => {
  const base = { ...settings, usage_reporting: { measured_visible: false } };
  const current = { ...base, usage_reporting: { measured_visible: true, future: 'current' } };
  expect(reviewSettingsDraft(base, base, current).usage_reporting).toEqual(current.usage_reporting);
  const edited = { ...base, usage_reporting: { measured_visible: true } };
  expect(reviewSettingsDraft(base, edited, current).usage_reporting).toEqual(
    current.usage_reporting,
  );
});
