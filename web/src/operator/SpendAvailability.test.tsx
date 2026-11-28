import { openSettingsEditors } from './settingsTestSupport';
import { readFileSync } from 'node:fs';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { ApiError, createClient } from '@cma/client';
import { PlatformSettings } from './PlatformSettings';
import { createOperatorClient } from './client';
import { operatorClient, settings, stored } from './testFixtures';
import { testBudget } from './spendTestFixtures';

const save = () => screen.getByRole('button', { name: 'Save settings' });
const amount = () => screen.getByLabelText('Per-turn cap (accounting units)');
const configured = (available?: boolean, enabled = false) => ({
  ...stored,
  model_budget_writes_available: available,
  settings: { ...settings, model_budget: { ...testBudget, enabled } },
});

for (const enabled of [false, true]) {
  it.each([false, undefined])(
    `blocks unrelated saves retaining cloud with enabled=${enabled} and platform availability %s`,
    async (available) => {
      const client = operatorClient();
      client.admin.readSettings.mockResolvedValue({
        ...configured(true, enabled),
        platform_spend_writes_available: available,
        settings: {
          ...settings,
          model_budget: {
            ...testBudget,
            enabled,
            platform: {
              ...testBudget.platform,
              cloud: { run_rate_units_per_day: 12, max_observation_age_seconds: 60 },
            },
          },
        },
      });
      render(<PlatformSettings client={client} />);
      await openSettingsEditors();
      await screen.findByLabelText('Enable spend controls');
      expect(amount()).toBeDisabled();
      fireEvent.change(screen.getByLabelText('Agent log retention (days)'), {
        target: { value: '45' },
      });
      expect(save()).toBeDisabled();
      fireEvent.submit(save().closest('form')!);
      expect(client.admin.replaceSettings).not.toHaveBeenCalled();
    },
  );
}

it.each([false, undefined])(
  'keeps retained policies readable but blocks all settings saves with availability %s',
  async (available) => {
    const client = operatorClient();
    const initial = configured(available);
    client.admin.readSettings.mockResolvedValue(initial);
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    await screen.findAllByText(/Saving spend controls is not available yet/);
    expect(screen.getByLabelText('Enable spend controls')).toBeDisabled();
    expect(amount()).toBeDisabled();
    expect(amount()).toHaveValue(String(testBudget.default_policy!.turn_limit));
    fireEvent.change(screen.getByLabelText('Agent log retention (days)'), {
      target: { value: '45' },
    });
    expect(save()).toBeDisabled();
    expect(
      screen.getByText(/Saving settings is not available yet for this spend policy/),
    ).toBeVisible();
    fireEvent.submit(save().closest('form')!);
    expect(client.admin.replaceSettings).not.toHaveBeenCalled();
    expect(amount()).toHaveValue(String(testBudget.default_policy!.turn_limit));
  },
);

it('keeps an enabled policy read-only when the deployment closes writes', async () => {
  const client = operatorClient();
  client.admin.readSettings.mockResolvedValue(configured(false, true));
  render(<PlatformSettings client={client} />);
  await openSettingsEditors();
  expect(await screen.findByLabelText('Enable spend controls')).toBeChecked();
  expect(screen.getByLabelText('Enable spend controls')).toBeDisabled();
  expect(client.admin.replaceSettings).not.toHaveBeenCalled();
});

it('retains a refused draft until a fresh available revision is explicitly reviewed', async () => {
  const client = operatorClient();
  const initial = configured(true);
  client.admin.readSettings.mockResolvedValueOnce(initial).mockResolvedValue(configured(false));
  client.admin.replaceSettings.mockRejectedValueOnce(
    new ApiError(503, 'Unavailable', 'model_budget_writes_unavailable'),
  );
  render(<PlatformSettings client={client} />);
  await openSettingsEditors();
  await screen.findByLabelText('Enable spend controls');
  fireEvent.change(amount(), { target: { value: '80' } });
  await userEvent.click(save());
  await screen.findByText(/The current revision is 4/);
  expect(screen.getByRole('alert')).toHaveTextContent('Saving spend controls is not available yet');
  expect(amount()).toHaveValue('80');
  expect(amount()).toBeDisabled();
  expect(save()).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Review retained draft' }));
  expect(save()).toBeDisabled();
  fireEvent.submit(save().closest('form')!);
  expect(client.admin.replaceSettings).toHaveBeenCalledTimes(1);
  client.admin.readSettings.mockResolvedValue({ ...initial, revision: 8 });
  client.admin.replaceSettings.mockResolvedValue({ ...initial, revision: 9 });
  await userEvent.click(screen.getByRole('button', { name: 'Refresh settings availability' }));
  await screen.findByText(/The current revision is 8/);
  await userEvent.click(screen.getByRole('button', { name: 'Review retained draft' }));
  expect(amount()).toHaveValue('80');
  await userEvent.click(save());
  await screen.findByText('Settings saved at revision 9.');
  expect(client.admin.replaceSettings.mock.calls[1]![0]).toMatchObject({
    expected_revision: 8,
    settings: { model_budget: { default_policy: { turn_limit: 80 } } },
  });
});

it.each([false, undefined])('adopts availability %s from a successful save', async (available) => {
  const client = operatorClient();
  const empty = { ...settings, model_budget: { enabled: false, owners: [] } };
  client.admin.readSettings.mockResolvedValue({ ...configured(true), settings: empty });
  client.admin.replaceSettings.mockResolvedValue({
    ...configured(available),
    settings: empty,
    revision: 5,
  });
  render(<PlatformSettings client={client} />);
  await openSettingsEditors();
  await screen.findByLabelText('Enable spend controls');
  await userEvent.click(save());
  await screen.findByText('Settings saved at revision 5.');
  expect(screen.getByLabelText('Enable spend controls')).toBeDisabled();
  expect(save()).toBeEnabled();
  expect(screen.getAllByText(/Saving spend controls is not available yet/)[0]).toBeVisible();
});

it('renders the actual backend public-contract response through the SDK with writes closed', async () => {
  const cases = JSON.parse(
    readFileSync('../api/crates/layer1-public/tests/fixtures/public-contract/admin.json', 'utf8'),
  ).cases as { id: string; response: { body: unknown } }[];
  const recorded = cases.find((entry) => entry.id === 'admin-settings-read')!.response.body;
  const writes: unknown[] = [];
  const sdk = createClient({
    baseUrl: 'https://fixture.example.test',
    fetch: async (_input, init) => {
      if (init?.method !== 'GET') writes.push(init?.body);
      return new Response(JSON.stringify(recorded), {
        headers: { 'content-type': 'application/json' },
      });
    },
  });
  render(<PlatformSettings client={createOperatorClient(sdk)} />);
  await openSettingsEditors();
  await screen.findAllByText(/Saving spend controls is not available yet/);
  expect(screen.getByLabelText('Enable spend controls')).not.toBeChecked();
  expect(screen.getByRole('button', { name: 'Configure platform period' })).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Configure default owner policy' })).toBeDisabled();
  await waitFor(() => expect(writes).toEqual([]));
});
