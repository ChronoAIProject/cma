import { openSettingsEditors } from './settingsTestSupport';
import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { createClient, type StoredPlatformSettings } from '@cma/client';
import { PlatformSettings } from './PlatformSettings';
import { createOperatorClient } from './client';
import { stored } from './testFixtures';
import { testBudget } from './spendTestFixtures';

const cloud = {
  run_rate_units_per_day: 12,
  max_observation_age_seconds: 60,
  future: { retained: true },
};
const configured = { ...testBudget, platform: { ...testBudget.platform!, cloud } };
const rateLabel = 'Cloud run rate (accounting units per day)';
const ageLabel = 'Maximum cloud observation age (seconds)';
const save = () => screen.getByRole('button', { name: 'Save settings' });
const change = (label: string, value: string) =>
  fireEvent.change(screen.getByLabelText(label), { target: { value } });

function fixture(budget: unknown = configured, available: boolean | undefined = true) {
  let current: StoredPlatformSettings = {
    ...stored,
    settings: { ...stored.settings, model_budget: budget },
    model_budget_writes_available: true,
    model_request_budget_writes_available: true,
    platform_spend_writes_available: available,
  };
  let failure = 0;
  const writes: { expected_revision: number; settings: StoredPlatformSettings['settings'] }[] = [];
  const client = createOperatorClient(
    createClient({
      baseUrl: 'https://fixture.example.test',
      fetch: async (_input, init) => {
        if (init?.method === 'PUT') {
          const body = JSON.parse(String(init.body)) as (typeof writes)[number];
          writes.push(body);
          if (failure)
            return Response.json(
              {
                status: failure,
                title: 'Settings refused',
                code:
                  failure === 503
                    ? 'model_budget_writes_unavailable'
                    : failure === 409
                      ? 'settings_conflict'
                      : 'invalid_request',
              },
              { status: failure },
            );
          current = { ...current, settings: body.settings, revision: current.revision + 1 };
        }
        return Response.json(current);
      },
    }),
  );
  return {
    client,
    writes,
    setAvailable: (value?: boolean) => {
      current = { ...current, platform_spend_writes_available: value };
    },
    setFailure: (value: number) => {
      failure = value;
    },
    advance: (budget: unknown) => {
      current = {
        ...current,
        revision: 8,
        settings: { ...current.settings, model_budget: budget },
      };
    },
  };
}

it('configures and removes the pair explicitly using exact integers and the current revision', async () => {
  const local = fixture(testBudget);
  render(<PlatformSettings client={local.client} />);
  await openSettingsEditors();
  await userEvent.click(await screen.findByRole('button', { name: 'Configure cloud metering' }));
  expect(screen.getByLabelText(rateLabel)).toHaveValue('');
  expect(screen.getByLabelText(ageLabel)).toHaveValue('');
  expect(screen.getByText('Projects daily cloud spend between observations.')).toBeVisible();
  change(rateLabel, '9007199254740991');
  fireEvent.submit(save().closest('form')!);
  expect(local.writes).toEqual([]);
  expect(screen.getByLabelText(ageLabel)).toHaveAttribute('aria-invalid', 'true');
  change(ageLabel, '60');
  await userEvent.click(save());
  await screen.findByText('Settings saved at revision 5.');
  expect(local.writes[0]).toMatchObject({
    expected_revision: 4,
    settings: {
      model_budget: {
        enabled: false,
        platform: {
          cloud: {
            run_rate_units_per_day: Number.MAX_SAFE_INTEGER,
            max_observation_age_seconds: 60,
          },
        },
      },
    },
  });
  expect(local.writes[0]).not.toHaveProperty('platform_spend_writes_available');
  await userEvent.click(screen.getByRole('button', { name: 'Remove cloud metering' }));
  await userEvent.click(save());
  await screen.findByText('Settings saved at revision 6.');
  expect(local.writes[1]).toMatchObject({
    expected_revision: 5,
    settings: { model_budget: testBudget },
  });
  expect(local.writes[1]!.settings.model_budget).not.toHaveProperty('platform.cloud');
});

it.each(['9007199254740991.1', '9007199254740992', '1e2', '0'])(
  'retains invalid cloud text %s without making a request',
  async (value) => {
    const local = fixture();
    render(<PlatformSettings client={local.client} />);
    await openSettingsEditors();
    await screen.findByLabelText(rateLabel);
    change(rateLabel, value);
    await userEvent.click(save());
    expect(screen.getByLabelText(rateLabel)).toHaveValue(value);
    expect(screen.getByLabelText(rateLabel)).toHaveAttribute('aria-invalid', 'true');
    expect(local.writes).toEqual([]);
  },
);

for (const enabled of [false, true]) {
  it.each([false, undefined])(
    `blocks retained cloud saves with enabled=${enabled} and platform availability %s`,
    async (available) => {
      const local = fixture({ ...configured, enabled });
      local.setAvailable(available);
      render(<PlatformSettings client={local.client} />);
      await openSettingsEditors();
      expect(await screen.findByLabelText(rateLabel)).toHaveValue('12');
      expect(screen.getByLabelText(rateLabel)).toBeDisabled();
      expect(screen.getByLabelText(ageLabel)).toBeDisabled();
      expect(screen.getByRole('button', { name: 'Remove cloud metering' })).toBeDisabled();
      change('Agent log retention (days)', '45');
      expect(save()).toBeDisabled();
      fireEvent.submit(save().closest('form')!);
      expect(local.writes).toEqual([]);
    },
  );
}

it.each([false, undefined])(
  'blocks new cloud configuration at availability %s without blocking other settings',
  async (available) => {
    const local = fixture(testBudget);
    local.setAvailable(available);
    render(<PlatformSettings client={local.client} />);
    await openSettingsEditors();
    expect(await screen.findByRole('button', { name: 'Configure cloud metering' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Refresh settings availability' })).toBeEnabled();
    change('Agent log retention (days)', '45');
    await userEvent.click(save());
    await screen.findByText('Settings saved at revision 5.');
    expect(local.writes[0]!.settings.model_budget).toEqual(testBudget);
  },
);

it('preserves unknown cloud data through unrelated and cloud edits', async () => {
  const local = fixture();
  render(<PlatformSettings client={local.client} />);
  await openSettingsEditors();
  await screen.findByLabelText(rateLabel);
  change('Agent log retention (days)', '45');
  change('Platform period cap (accounting units)', '20000');
  await userEvent.click(save());
  await screen.findByText('Settings saved at revision 5.');
  expect(local.writes[0]!.settings.model_budget).toHaveProperty('platform.cloud', cloud);
  change(rateLabel, '24');
  await userEvent.click(save());
  await screen.findByText('Settings saved at revision 6.');
  expect(local.writes[1]!.settings.model_budget).toHaveProperty('platform.cloud', {
    ...cloud,
    run_rate_units_per_day: 24,
  });
});

it('preserves unsupported cloud data on an unrelated settings save', async () => {
  const budget = {
    ...testBudget,
    platform: { ...testBudget.platform, cloud: { future: 'opaque' } },
  };
  const local = fixture(budget);
  render(<PlatformSettings client={local.client} />);
  await openSettingsEditors();
  await screen.findAllByText(
    /This server does not expose the supported spend-controls configuration/,
  );
  change('Agent log retention (days)', '45');
  await userEvent.click(save());
  await screen.findByText('Settings saved at revision 5.');
  expect(local.writes[0]!.settings.model_budget).toEqual(budget);
});

it('retains a rejected cloud draft on validation failure', async () => {
  const local = fixture();
  local.setFailure(422);
  render(<PlatformSettings client={local.client} />);
  await openSettingsEditors();
  await screen.findByLabelText(rateLabel);
  change(rateLabel, '24');
  await userEvent.click(save());
  expect(await screen.findByRole('alert')).toHaveTextContent('Review the settings fields');
  expect(screen.getByLabelText(rateLabel)).toHaveValue('24');
  expect(local.writes).toHaveLength(1);
});

it('reviews cloud edits against the latest revision without overwriting new unknown fields', async () => {
  const local = fixture();
  render(<PlatformSettings client={local.client} />);
  await openSettingsEditors();
  await screen.findByLabelText(rateLabel);
  change(rateLabel, '24');
  const latestCloud = { ...cloud, max_observation_age_seconds: 90, future: { retained: false } };
  local.advance({ ...configured, platform: { ...configured.platform, cloud: latestCloud } });
  local.setFailure(409);
  await userEvent.click(save());
  await screen.findByText(/The current revision is 8/);
  expect(screen.getByLabelText(rateLabel)).toHaveValue('24');
  expect(save()).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Review retained draft' }));
  expect(screen.getByLabelText(ageLabel)).toHaveValue('90');
  local.setFailure(0);
  await userEvent.click(save());
  await screen.findByText('Settings saved at revision 9.');
  expect(local.writes[1]).toMatchObject({
    expected_revision: 8,
    settings: {
      model_budget: { platform: { cloud: { ...latestCloud, run_rate_units_per_day: 24 } } },
    },
  });
});

it.each([false, undefined])(
  'keeps a refused draft closed until platform availability %s is refreshed and reviewed',
  async (available) => {
    const local = fixture();
    render(<PlatformSettings client={local.client} />);
    await openSettingsEditors();
    await screen.findByLabelText(rateLabel);
    change(rateLabel, '24');
    local.setAvailable(available);
    local.setFailure(503);
    await userEvent.click(save());
    await screen.findByText(/The current revision is 4/);
    expect(screen.getByLabelText(rateLabel)).toHaveValue('24');
    expect(screen.getByLabelText(rateLabel)).toBeDisabled();
    await userEvent.click(screen.getByRole('button', { name: 'Review retained draft' }));
    expect(save()).toBeDisabled();
    fireEvent.submit(save().closest('form')!);
    expect(local.writes).toHaveLength(1);
    local.setAvailable(true);
    local.setFailure(0);
    await userEvent.click(screen.getByRole('button', { name: 'Refresh settings availability' }));
    await screen.findByText(/The current revision is 4/);
    expect(save()).toBeDisabled();
    await userEvent.click(screen.getByRole('button', { name: 'Review retained draft' }));
    expect(screen.getByLabelText(rateLabel)).toHaveValue('24');
    await userEvent.click(save());
    await screen.findByText('Settings saved at revision 5.');
    expect(local.writes[1]!.settings.model_budget).toHaveProperty(
      'platform.cloud.run_rate_units_per_day',
      24,
    );
  },
);
