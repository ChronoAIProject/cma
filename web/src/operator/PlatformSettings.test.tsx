import { openSettingsEditors } from './settingsTestSupport';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { ApiError, type StoredPlatformSettings } from '@cma/client';
import { PlatformSettings } from './PlatformSettings';
import { operatorClient, settings, stored } from './testFixtures';
import { reviewSettingsDraft } from './settingsDraft';

describe('platform settings', () => {
  it('does not expose CMA snapshot scheduling or retired profile configuration', async () => {
    const client = operatorClient();
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    await screen.findByLabelText('Pause after (seconds)');
    expect(screen.queryByText('Snapshots')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Save progress every (minutes)')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Checkpoint grace (days)')).not.toBeInTheDocument();
    expect(screen.queryByLabelText(/Default .* profile/)).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Marketplace enabled')).not.toBeInTheDocument();
    expect(client.listPublicProfiles).not.toHaveBeenCalled();
  });
  it('rebases an explicitly selected profile and revision as one atomic reference', () => {
    const current = {
      ...settings,
      defaults: { ...settings.defaults, agent_profile: { id: 'agp_default', revision: 4 } },
    };
    const draft = {
      ...settings,
      defaults: { ...settings.defaults, agent_profile: { id: 'new-agent', revision: 3 } },
    };
    expect(reviewSettingsDraft(settings, draft, current).defaults.agent_profile).toEqual({
      id: 'new-agent',
      revision: 3,
    });
  });
  it('saves counts and distinct toggles while preserving loaded compatibility values', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    const pause = await screen.findByLabelText('Pause after (seconds)');
    await user.clear(pause);
    await user.type(pause, '600');
    await user.click(screen.getByLabelText('Triggers enabled'));
    await user.click(screen.getByLabelText('Orchestration paused'));
    await user.click(screen.getByRole('button', { name: 'Save settings' }));
    await screen.findByText('Settings saved at revision 5.');
    expect(client.admin.replaceSettings).toHaveBeenCalledWith({
      expected_revision: 4,
      settings: {
        ...settings,
        inactivity: { pause_after_seconds: 600 },
        triggers: { enabled: false, orchestration_paused: true },
      },
    });
  });

  it('shows one hours/minutes/seconds threshold when auto pause is enabled', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    await screen.findByLabelText('Enable auto pause');
    expect(screen.queryByLabelText('Pause after hours')).not.toBeInTheDocument();
    await user.click(screen.getByLabelText('Enable auto pause'));
    expect(screen.getByLabelText('Pause after hours')).toHaveValue(0);
    expect(screen.getByLabelText('Pause after minutes')).toHaveValue(15);
    expect(screen.getByLabelText('Pause after seconds')).toHaveValue(0);
    expect(screen.queryByLabelText('Pause after (seconds)')).not.toBeInTheDocument();
    await user.clear(screen.getByLabelText('Pause after minutes'));
    await user.type(screen.getByLabelText('Pause after minutes'), '30');
    await user.click(screen.getByRole('button', { name: 'Save settings' }));
    await screen.findByText('Settings saved at revision 5.');
    expect(client.admin.replaceSettings).toHaveBeenCalledWith({
      expected_revision: 4,
      settings: {
        ...settings,
        inactivity: { pause_after_seconds: 1800, auto_pause_enabled: true },
      },
    });
  });

  it('retains the draft on conflict and rebases only edited values without automatically writing', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    const current: StoredPlatformSettings = {
      revision: 8,
      settings: {
        ...settings,
        future: { preserved: 'newer server value' },
        retention: { ...settings.retention, agent_log_days: 40 },
        inactivity: { pause_after_seconds: 700 },
        defaults: { ...settings.defaults, sandbox_profile: { id: 'new-default', revision: 9 } },
        marketplace: { enabled: false },
      },
    };
    client.admin.readSettings.mockResolvedValueOnce(stored).mockResolvedValue(current);
    client.admin.replaceSettings.mockRejectedValueOnce(
      new ApiError(409, 'Conflict', 'settings_conflict'),
    );
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    const pause = await screen.findByLabelText('Pause after (seconds)');
    await user.clear(pause);
    await user.type(pause, '600');
    await user.click(screen.getByRole('button', { name: 'Save settings' }));
    await screen.findByText(/The current revision is 8/);
    expect(pause).toHaveValue(600);
    expect(screen.getByRole('button', { name: 'Save settings' })).toBeDisabled();
    expect(client.admin.replaceSettings).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole('button', { name: 'Review retained draft' }));
    expect(pause).toHaveValue(600);
    expect(screen.getByLabelText('Agent log retention (days)')).toHaveValue(40);
    expect(client.admin.replaceSettings).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole('button', { name: 'Save settings' }));
    await screen.findByText('Settings saved at revision 9.');
    expect(client.admin.replaceSettings).toHaveBeenLastCalledWith({
      expected_revision: 8,
      settings: { ...current.settings, inactivity: { pause_after_seconds: 600 } },
    });
  });

  it('can explicitly discard a conflicted draft and load current values', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    client.admin.readSettings.mockResolvedValueOnce(stored).mockResolvedValue({
      revision: 6,
      settings: { ...settings, inactivity: { pause_after_seconds: 750 } },
    });
    client.admin.replaceSettings.mockRejectedValue(
      new ApiError(409, 'Conflict', 'settings_conflict'),
    );
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    const pause = await screen.findByLabelText('Pause after (seconds)');
    await user.clear(pause);
    await user.type(pause, '600');
    await user.click(screen.getByRole('button', { name: 'Save settings' }));
    await screen.findByText(/current revision is 6/);
    await user.click(screen.getByRole('button', { name: 'Load current values' }));
    expect(pause).toHaveValue(750);
    expect(screen.getByText('Revision 6')).toBeInTheDocument();
    expect(client.admin.replaceSettings).toHaveBeenCalledTimes(1);
  });

  it('retains unknown writes, permits a failed current read to be retried, and never resubmits automatically', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    client.admin.readSettings
      .mockResolvedValueOnce(stored)
      .mockRejectedValueOnce(new Error('Disconnected'))
      .mockResolvedValue({ ...stored, revision: 5 });
    client.admin.replaceSettings.mockRejectedValue(new Error('Disconnected'));
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    await screen.findByLabelText('Pause after (seconds)');
    await user.click(screen.getByRole('button', { name: 'Save settings' }));
    await screen.findByText(/current settings could not be read/);
    expect(screen.getByRole('button', { name: 'Save settings' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Read current settings' }));
    await screen.findByText(/current revision is 5/);
    expect(client.admin.replaceSettings).toHaveBeenCalledTimes(1);
  });

  it('suppresses duplicate saves, keeps controls stable, and preserves server refusal paths beside fields', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    let reject!: (reason: unknown) => void;
    client.admin.replaceSettings.mockReturnValue(
      new Promise((_, fail) => {
        reject = fail;
      }),
    );
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    await screen.findByLabelText('Sandboxes per user', { selector: 'input' });
    const save = screen.getByRole('button', { name: 'Save settings' });
    await user.dblClick(save);
    expect(client.admin.replaceSettings).toHaveBeenCalledTimes(1);
    expect(screen.getByLabelText('Sandboxes per user', { selector: 'input' })).toBeDisabled();
    await act(async () =>
      reject(
        new ApiError(400, 'Too many Sandboxes', 'invalid_request', 'req-1', undefined, {
          path: 'settings.limits.max_sandboxes_per_user',
        }),
      ),
    );
    await waitFor(() =>
      expect(screen.getByLabelText('Sandboxes per user', { selector: 'input' })).toHaveAttribute(
        'aria-invalid',
        'true',
      ),
    );
    expect(
      screen.getByLabelText('Sandboxes per user', { selector: 'input' }),
    ).toHaveAccessibleDescription('Review the settings fields before saving again.');
    expect(screen.getByText('req-1')).toBeInTheDocument();
    expect(screen.getByLabelText('Sandboxes per user', { selector: 'input' })).toHaveValue(10);
  });

  it('bounds numeric values and does not save an invalid native form', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    const pause = await screen.findByLabelText('Pause after (seconds)');
    fireEvent.change(pause, { target: { value: '59' } });
    expect(pause).toBeInvalid();
    await user.click(screen.getByRole('button', { name: 'Save settings' }));
    expect(client.admin.replaceSettings).not.toHaveBeenCalled();
    fireEvent.change(pause, { target: { value: '' } });
    expect(pause).toBeInvalid();
    fireEvent.change(screen.getByLabelText('Visitor launches per day', { selector: 'input' }), {
      target: { value: '4294967296' },
    });
    expect(screen.getByLabelText('Visitor launches per day', { selector: 'input' })).toBeInvalid();
  });

  it('retains exact defaults without querying the retired marketplace', async () => {
    const client = operatorClient();
    const user = userEvent.setup();
    client.listPublicProfiles.mockRejectedValue(
      new ApiError(403, 'Disabled', 'marketplace_disabled'),
    );
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    await screen.findByLabelText('Sandboxes per user', { selector: 'input' });
    expect(client.listPublicProfiles).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Save settings' }));
    await screen.findByText('Settings saved at revision 5.');
    expect(client.admin.replaceSettings).toHaveBeenCalledWith({ expected_revision: 4, settings });
  });

  it('retains newer root, section, and reference fields while reviewing edited values', () => {
    const current = {
      ...settings,
      future: 'current',
      inactivity: { ...settings.inactivity, future_seconds: 50 },
      defaults: {
        ...settings.defaults,
        agent_profile: { ...settings.defaults.agent_profile, metadata: 'current' },
      },
    };
    const draft = {
      ...settings,
      inactivity: { pause_after_seconds: 600 },
      defaults: { ...settings.defaults, agent_profile: { id: 'new-profile', revision: 7 } },
    };
    expect(reviewSettingsDraft(settings, draft, current)).toEqual({
      ...current,
      inactivity: { pause_after_seconds: 600, future_seconds: 50 },
      defaults: {
        ...current.defaults,
        agent_profile: { id: 'new-profile', revision: 7, metadata: 'current' },
      },
    });
  });
});
