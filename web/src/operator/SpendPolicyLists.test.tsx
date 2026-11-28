import { openSettingsEditors } from './settingsTestSupport';
import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { ApiError } from '@cma/client';
import { PlatformSettings } from './PlatformSettings';
import { operatorClient, settings, stored } from './testFixtures';
import { ownerAmounts } from './spendBudget';
import { testBudget } from './spendTestFixtures';

function setup(requestAvailable: boolean | undefined = true) {
  const client = operatorClient();
  const budget = { ...structuredClone(testBudget), owners: [], request_models: [] };
  const initial = {
    ...stored,
    model_budget_writes_available: true,
    model_request_budget_writes_available: requestAvailable,
    settings: { ...settings, model_budget: budget },
  };
  client.admin.readSettings.mockResolvedValue(initial);
  client.admin.replaceSettings.mockImplementation(async (body) => ({
    ...initial,
    revision: body.expected_revision + 1,
    settings: body.settings,
  }));
  return { client, budget, initial, user: userEvent.setup() };
}
const change = (label: string, value: string) =>
  fireEvent.change(screen.getByLabelText(label), { target: { value } });
const save = () => screen.getByRole('button', { name: 'Save settings' });

async function addOwner(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole('button', { name: 'Add owner override' }));
  change('Owner override 1: owner ID', 'fixture-owner');
  for (const [field, label] of Object.entries(ownerAmounts))
    change(
      `Owner override 1: ${label}`,
      String(testBudget.default_policy![field as keyof typeof ownerAmounts]),
    );
  change('Owner override 1: Day-boundary timezone (IANA)', testBudget.default_policy!.day_timezone);
}
async function addModel(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole('button', { name: 'Add request model' }));
  change('Request model 1: model name', 'fixture-model');
  change('Request model 1: input overhead (tokens)', '1');
}

it('saves explicit owner and model policies with CAS, keeping enforcement disabled', async () => {
  const { client, user, budget } = setup();
  render(<PlatformSettings client={client} />);
  await openSettingsEditors();
  await screen.findByLabelText('Enable spend controls');
  await addOwner(user);
  await addModel(user);
  await user.click(screen.getByRole('button', { name: 'Remove default owner policy' }));
  await user.click(save());
  await screen.findByText('Settings saved at revision 5.');
  expect(client.admin.replaceSettings).toHaveBeenCalledWith({
    expected_revision: 4,
    settings: {
      ...settings,
      model_budget: {
        ...budget,
        default_policy: undefined,
        owners: [{ owner: 'fixture-owner', policy: testBudget.default_policy }],
        request_models: [{ model: 'fixture-model', input_overhead_tokens: 1 }],
      },
    },
  });
  expect(screen.getByLabelText('Enable spend controls')).not.toBeChecked();
  await user.click(screen.getByRole('button', { name: 'Remove owner override 1' }));
  await user.click(screen.getByRole('button', { name: 'Remove request model 1' }));
  await user.click(save());
  await screen.findByText('Settings saved at revision 6.');
  expect(client.admin.replaceSettings.mock.calls[1]![0]).toMatchObject({
    expected_revision: 5,
    settings: { model_budget: { enabled: false, owners: [], request_models: [] } },
  });
});

it('starts added rows blank and retains invalid inputs without a write', async () => {
  const { client, user } = setup();
  render(<PlatformSettings client={client} />);
  await openSettingsEditors();
  await user.click(await screen.findByRole('button', { name: 'Add owner override' }));
  await user.click(screen.getByRole('button', { name: 'Add request model' }));
  for (const name of ['Owner override 1', 'Request model 1'])
    for (const input of within(screen.getByRole('group', { name })).getAllByRole('textbox'))
      expect(input).toHaveValue('');
  change('Request model 1: input overhead (tokens)', '1.5');
  await user.click(save());
  expect(client.admin.replaceSettings).not.toHaveBeenCalled();
  expect(screen.getByLabelText('Owner override 1: owner ID')).toBeInvalid();
  // Native required fields block clicking Save; direct submits still validate every row.
  fireEvent.submit(save().closest('form')!);
  expect(screen.getByLabelText('Owner override 1: owner ID')).toHaveAttribute(
    'aria-invalid',
    'true',
  );
  expect(screen.getByLabelText('Request model 1: input overhead (tokens)')).toHaveValue('1.5');
  expect(screen.getByLabelText('Request model 1: input overhead (tokens)')).toHaveAttribute(
    'aria-invalid',
    'true',
  );
});

it.each([false, undefined])(
  'keeps empty request editing unavailable at gate %s while owner policies can save',
  async (gate) => {
    const { client, user, initial } = setup(gate);
    // Passing undefined must model absent metadata, not the helper's default.
    client.admin.readSettings.mockResolvedValue({
      ...initial,
      model_request_budget_writes_available: gate,
    });
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    await screen.findByLabelText('Enable spend controls');
    expect(screen.getByRole('button', { name: 'Add request model' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Refresh settings availability' })).toBeEnabled();
    await addOwner(user);
    await user.click(save());
    await screen.findByText('Settings saved at revision 5.');
    expect(client.admin.replaceSettings).toHaveBeenCalledTimes(1);
  },
);

it.each(['owners', 'request_models'] as const)(
  'retains a concurrently changed %s draft until current values are explicitly loaded',
  async (key) => {
    const { client, user, initial } = setup();
    const latest = {
      ...initial,
      revision: 9,
      settings: {
        ...initial.settings,
        model_budget: {
          ...initial.settings.model_budget,
          owners:
            key === 'owners'
              ? [{ owner: 'fixture-other', policy: testBudget.default_policy! }]
              : [],
          request_models:
            key === 'request_models' ? [{ model: 'fixture-other', input_overhead_tokens: 2 }] : [],
        },
      },
    };
    client.admin.readSettings.mockResolvedValueOnce(initial).mockResolvedValue(latest);
    client.admin.replaceSettings.mockRejectedValueOnce(
      new ApiError(409, 'Conflict', 'settings_conflict'),
    );
    render(<PlatformSettings client={client} />);
    await openSettingsEditors();
    await screen.findByLabelText('Enable spend controls');
    if (key === 'owners') await addOwner(user);
    else await addModel(user);
    await user.click(save());
    await screen.findByText(/The current revision is 9/);
    expect(screen.getByText(/changed both here and on the server/)).toBeVisible();
    expect(screen.getByRole('button', { name: 'Review retained draft' })).toBeDisabled();
    expect(save()).toBeDisabled();
    expect(
      screen.getByLabelText(
        key === 'owners' ? 'Owner override 1: owner ID' : 'Request model 1: model name',
      ),
    ).toHaveValue(key === 'owners' ? 'fixture-owner' : 'fixture-model');
    fireEvent.submit(save().closest('form')!);
    expect(client.admin.replaceSettings).toHaveBeenCalledTimes(1);
    await user.click(screen.getByRole('button', { name: 'Load current values' }));
    expect(
      screen.getByLabelText(
        key === 'owners' ? 'Owner override 1: owner ID' : 'Request model 1: model name',
      ),
    ).toHaveValue('fixture-other');
    expect(save()).toBeEnabled();
  },
);

it('retains new model edits after stale availability closes and only saves after explicit fresh review', async () => {
  const { client, user, initial } = setup();
  client.admin.readSettings
    .mockResolvedValueOnce(initial)
    .mockResolvedValue({ ...initial, model_request_budget_writes_available: false });
  client.admin.replaceSettings.mockRejectedValueOnce(
    new ApiError(503, 'Unavailable', 'model_budget_writes_unavailable'),
  );
  render(<PlatformSettings client={client} />);
  await openSettingsEditors();
  await screen.findByLabelText('Enable spend controls');
  await addModel(user);
  await user.click(save());
  await screen.findByText(/The current revision is 4/);
  await user.click(screen.getByRole('button', { name: 'Review retained draft' }));
  expect(screen.getByLabelText('Request model 1: model name')).toHaveValue('fixture-model');
  expect(save()).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Remove request model 1' })).toBeDisabled();
  client.admin.readSettings.mockResolvedValue({ ...initial, revision: 8 });
  await user.click(screen.getByRole('button', { name: 'Refresh settings availability' }));
  await screen.findByText(/The current revision is 8/);
  await user.click(screen.getByRole('button', { name: 'Review retained draft' }));
  await user.click(save());
  await screen.findByText('Settings saved at revision 9.');
  expect(client.admin.replaceSettings.mock.calls[1]![0]).toMatchObject({
    expected_revision: 8,
    settings: {
      model_budget: { request_models: [{ model: 'fixture-model', input_overhead_tokens: 1 }] },
    },
  });
});
