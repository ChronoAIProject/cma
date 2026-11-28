import { openSettingsEditors } from './settingsTestSupport';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it } from 'vitest';
import { ApiError } from '@cma/client';
import { PlatformSettings } from './PlatformSettings';
import { Operations } from './Operations';
import { operatorClient, settings, stored } from './testFixtures';
import { testBudget } from './spendTestFixtures';

function setup() {
  const client = operatorClient();
  const budget = structuredClone(testBudget);
  budget.owners = [
    { owner: 'fixture-owner', policy: { ...budget.default_policy!, turn_limit: 60 } },
  ];
  const initial = {
    ...stored,
    model_budget_writes_available: true,
    settings: { ...settings, model_budget: budget },
  };
  client.admin.readSettings.mockResolvedValue(initial);
  client.admin.replaceSettings.mockImplementation(async (body) => ({
    revision: body.expected_revision + 1,
    settings: body.settings,
    model_budget_writes_available: true,
  }));
  return { client, budget, initial, user: userEvent.setup() };
}
const change = (label: string, value: string) =>
  fireEvent.change(screen.getByLabelText(label), { target: { value } });
const save = () => screen.getByRole('button', { name: 'Save settings' });

describe('admin spend controls', () => {
  it('saves configured units with the loaded revision and preserves policies when disabled', async () => {
    const { client, budget, initial, user } = setup();
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    const enabled = await screen.findByLabelText('Enable spend controls');
    expect(enabled).not.toBeChecked();
    expect(screen.getByLabelText('Day-boundary timezone (IANA)')).toHaveValue('Asia/Singapore');
    expect(screen.getAllByText('1 configured').length).toBeGreaterThan(0);
    await user.click(enabled);
    change('Per-turn cap (accounting units)', '80');
    await user.click(save());
    await screen.findByText('Settings saved at revision 5.');
    const policy = {
      ...budget,
      enabled: true,
      default_policy: { ...budget.default_policy!, turn_limit: 80 },
    };
    expect(client.admin.replaceSettings).toHaveBeenLastCalledWith({
      expected_revision: 4,
      settings: { ...initial.settings, model_budget: policy },
    });
    await user.click(enabled);
    await user.click(save());
    await screen.findByText('Settings saved at revision 6.');
    expect(client.admin.replaceSettings).toHaveBeenLastCalledWith({
      expected_revision: 5,
      settings: { ...initial.settings, model_budget: { ...policy, enabled: false } },
    });
    expect(client.admin.listActivity).not.toHaveBeenCalled();
  });
  it('refuses precision loss and cross-field validation before writing', async () => {
    const { client, user } = setup();
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    await screen.findByLabelText('Enable spend controls');
    change('Platform period cap (accounting units)', '9007199254740991.1');
    change('Per-turn cap (accounting units)', '1001');
    await user.click(save());
    expect(client.admin.replaceSettings).not.toHaveBeenCalled();
    expect(screen.getByLabelText('Platform period cap (accounting units)')).toHaveAttribute(
      'aria-invalid',
      'true',
    );
    expect(screen.getByText('The turn cap must not exceed the daily cap.')).toBeInTheDocument();
  });
  it('starts empty policies without inventing amounts, percentages or a timezone', async () => {
    const { client, user } = setup();
    client.admin.readSettings.mockResolvedValue({
      ...stored,
      model_budget_writes_available: true,
      settings: { ...settings, model_budget: { enabled: false, owners: [] } },
    });
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    await user.click(await screen.findByLabelText('Enable spend controls'));
    await user.click(save());
    expect(await screen.findByRole('alert')).toHaveTextContent('Enabling requires');
    expect(client.admin.replaceSettings).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Configure platform period' }));
    await user.click(screen.getByRole('button', { name: 'Configure default owner policy' }));
    expect(screen.getByLabelText('Platform period cap (accounting units)')).toHaveValue('');
    expect(screen.getByLabelText('Warning threshold (%)')).toHaveValue('');
    expect(screen.getByLabelText('Day-boundary timezone (IANA)')).toHaveValue('');
  });
  it.each([409, 503])(
    'retains a draft after %s and requires explicit review using the new revision',
    async (status) => {
      const { client, budget, initial, user } = setup();
      const nextBudget = {
        ...budget,
        owners: [...budget.owners, { owner: 'second-fixture', policy: budget.default_policy! }],
        platform: { ...budget.platform!, warning_percent: 50 },
      };
      const current = {
        revision: 8,
        model_budget_writes_available: true,
        settings: {
          ...initial.settings,
          retention: { agent_log_days: 45 },
          model_budget: nextBudget,
        },
      };
      client.admin.readSettings.mockResolvedValueOnce(initial).mockResolvedValue(current);
      client.admin.replaceSettings.mockRejectedValueOnce(
        new ApiError(status, 'Save refused', status === 409 ? 'settings_conflict' : 'unavailable'),
      );
      render(<PlatformSettings client={client} />);
      await openSettingsEditors();
      await screen.findByLabelText('Enable spend controls');
      change('Per-turn cap (accounting units)', '80');
      await user.click(save());
      await screen.findByText(/The current revision is 8/);
      expect(save()).toBeDisabled();
      expect(screen.getByLabelText('Per-turn cap (accounting units)')).toHaveValue('80');
      expect(client.admin.replaceSettings).toHaveBeenCalledTimes(1);
      await user.click(screen.getByRole('button', { name: 'Review retained draft' }));
      expect(screen.getByLabelText('Warning threshold (%)')).toHaveValue('50');
      await user.click(save());
      await screen.findByText('Settings saved at revision 9.');
      expect(client.admin.replaceSettings).toHaveBeenLastCalledWith({
        expected_revision: 8,
        settings: {
          ...current.settings,
          model_budget: {
            ...nextBudget,
            default_policy: { ...budget.default_policy!, turn_limit: 80 },
          },
        },
      });
    },
  );
  it('retains a conflicted draft when the current server no longer has this contract', async () => {
    const { client, initial, user } = setup();
    client.admin.readSettings
      .mockResolvedValueOnce(initial)
      .mockResolvedValue({ ...stored, revision: 9 });
    client.admin.replaceSettings.mockRejectedValueOnce(
      new ApiError(409, 'Conflict', 'settings_conflict'),
    );
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    await user.click(await screen.findByLabelText('Enable spend controls'));
    await user.click(save());
    await screen.findByText(/The current server no longer exposes/);
    expect(screen.getByRole('button', { name: 'Review retained draft' })).toBeDisabled();
    expect(save()).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Load current values' }));
    expect(screen.queryByLabelText('Enable spend controls')).not.toBeInTheDocument();
    expect(
      screen.getAllByText(/This server does not expose the supported spend-controls configuration/)
        .length,
    ).toBeGreaterThan(0);
  });
  it.each([403, 422, 429])(
    'shows an explicit %s refusal without a saved notice or automatic retry',
    async (status) => {
      const { client, user } = setup();
      client.admin.replaceSettings.mockRejectedValue(
        new ApiError(
          status,
          'Server refused the update',
          status === 422 ? 'invalid_request' : undefined,
        ),
      );
      render(<PlatformSettings client={client} />);
      await openSettingsEditors();
      await user.click(await screen.findByLabelText('Enable spend controls'));
      await user.click(save());
      await screen.findByRole('alert');
      await waitFor(() => expect(save()).toBeEnabled());
      expect(screen.queryByText(/Settings saved/)).not.toBeInTheDocument();
      expect(client.admin.replaceSettings).toHaveBeenCalledTimes(1);
      expect(screen.getByLabelText('Enable spend controls')).toBeChecked();
    },
  );
  it('disables edits while a save is pending', async () => {
    const { client, user, initial } = setup();
    let finish!: (value: typeof initial) => void;
    client.admin.replaceSettings.mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    await screen.findByLabelText('Enable spend controls');
    await user.click(save());
    expect(screen.getByLabelText('Per-turn cap (accounting units)')).toBeDisabled();
    expect(screen.getByLabelText('Enable spend controls')).toBeDisabled();
    await act(async () => finish(initial));
  });
  it('does not expose spend configuration to an Operations reader without ConfigurePlatform', async () => {
    const { client } = setup();
    render(<Operations client={client} capabilities={['read_activity_log']} />);
    await screen.findByRole('tab', { name: 'Activity' });
    expect(screen.queryByRole('tab', { name: 'Platform settings' })).not.toBeInTheDocument();
    expect(screen.queryByText('Spend controls')).not.toBeInTheDocument();
    expect(client.admin.readSettings).not.toHaveBeenCalled();
  });
});
