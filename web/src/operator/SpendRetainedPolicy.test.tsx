import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { createClient, type StoredPlatformSettings } from '@cma/client';
import { PlatformSettings } from './PlatformSettings';
import { createOperatorClient } from './client';
import { stored } from './testFixtures';
import { testBudget } from './spendTestFixtures';

const models = [{ model: 'fixture-model', input_overhead_tokens: 1 }];
const retained = {
  ...testBudget,
  owners: [{ owner: 'fixture-owner', policy: testBudget.default_policy! }],
  request_models: models,
};
const save = () => screen.getByRole('button', { name: 'Save settings' });
const amount = () => screen.getByLabelText('Per-turn cap (accounting units)');

// Exercise the real SDK's metadata decoding and request serialization.
function transport(budget: unknown, foundation?: boolean, request?: boolean) {
  let current: StoredPlatformSettings = {
    ...stored,
    settings: { ...stored.settings, model_budget: budget },
    model_budget_writes_available: foundation,
    model_request_budget_writes_available: request,
  };
  const writes: { expected_revision: number; settings: StoredPlatformSettings['settings'] }[] = [];
  let refuse = false;
  const sdk = createClient({
    baseUrl: 'https://fixture.example.test',
    fetch: async (_input, init) => {
      if (init?.method === 'PUT') {
        const body = JSON.parse(String(init.body)) as (typeof writes)[number];
        writes.push(body);
        if (refuse)
          return Response.json(
            { status: 503, code: 'model_budget_writes_unavailable', title: 'Unavailable' },
            { status: 503 },
          );
        current = {
          ...current,
          revision: current.revision + 1,
          settings: {
            ...body.settings,
            model_budget:
              body.settings.model_budget === undefined
                ? current.settings.model_budget
                : body.settings.model_budget,
          },
        };
      }
      return Response.json(current);
    },
  });
  return {
    client: createOperatorClient(sdk),
    writes,
    gates: (foundation?: boolean, request?: boolean) => {
      current = {
        ...current,
        model_budget_writes_available: foundation,
        model_request_budget_writes_available: request,
      };
    },
    refuse: (value: boolean) => {
      refuse = value;
    },
  };
}

for (const enabled of [false, true]) {
  it.each([
    [true, false],
    [true, undefined],
    [false, true],
    [undefined, true],
  ])(
    'blocks retained request policy with gates %s/%s, enabled=' + enabled,
    async (foundation, request) => {
      const local = transport({ ...retained, enabled }, foundation, request);
      render(<PlatformSettings client={local.client} />);
      await screen.findByLabelText('Enable spend controls');
      expect(amount()).toBeDisabled();
      expect(amount()).toHaveValue(String(testBudget.default_policy!.turn_limit));
      fireEvent.change(screen.getByLabelText('Agent log retention (days)'), {
        target: { value: '45' },
      });
      expect(save()).toBeDisabled();
      fireEvent.submit(save().closest('form')!);
      expect(local.writes).toEqual([]);
    },
  );
}

it.each([undefined, []].map((request_models) => ({ request_models })))(
  'allows foundation writes without request models $request_models',
  async ({ request_models }) => {
    const local = transport({ ...testBudget, request_models }, true, false);
    render(<PlatformSettings client={local.client} />);
    await screen.findByLabelText('Enable spend controls');
    expect(amount()).toBeEnabled();
    fireEvent.change(amount(), { target: { value: '80' } });
    await userEvent.click(save());
    await screen.findByText('Settings saved at revision 5.');
    expect(local.writes[0]!.settings.model_budget).toMatchObject({
      default_policy: { turn_limit: 80 },
    });
  },
);

it.each([
  undefined,
  { enabled: false, owners: [] },
  { enabled: false, owners: [], request_models: [] },
])(
  'allows unrelated saves with an absent/empty disabled budget and closed gates: %j',
  async (budget) => {
    const local = transport(budget, false, false);
    render(<PlatformSettings client={local.client} />);
    await screen.findByLabelText('Agent log retention (days)');
    fireEvent.change(screen.getByLabelText('Agent log retention (days)'), {
      target: { value: '45' },
    });
    expect(save()).toBeEnabled();
    await userEvent.click(save());
    await screen.findByText('Settings saved at revision 5.');
    expect(local.writes[0]!.settings.retention).toEqual({ agent_log_days: 45 });
  },
);

it.each([
  { platform: testBudget.platform },
  { default_policy: testBudget.default_policy },
  { owners: retained.owners },
  { request_models: models },
])('requires the foundation gate for each retained policy section: %j', async (policy) => {
  const local = transport({ enabled: false, owners: [], ...policy }, false, true);
  render(<PlatformSettings client={local.client} />);
  await screen.findByLabelText('Enable spend controls');
  expect(save()).toBeDisabled();
  fireEvent.submit(save().closest('form')!);
  expect(local.writes).toEqual([]);
});

it.each([false, undefined])(
  'does not reopen retained policy after reviewing request availability %s',
  async (request) => {
    const local = transport(retained, true, true);
    render(<PlatformSettings client={local.client} />);
    await screen.findByLabelText('Enable spend controls');
    fireEvent.change(amount(), { target: { value: '80' } });
    local.gates(true, request);
    local.refuse(true);
    await userEvent.click(save());
    await screen.findByText(/The current revision is 4/);
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Saving spend controls is not available yet',
    );
    expect(amount()).toHaveValue('80');
    expect(amount()).toBeDisabled();
    await userEvent.click(screen.getByRole('button', { name: 'Review retained draft' }));
    expect(save()).toBeDisabled();
    expect(amount()).toBeDisabled();
    fireEvent.submit(save().closest('form')!);
    expect(local.writes).toHaveLength(1);
    local.gates(true, true);
    local.refuse(false);
    await userEvent.click(screen.getByRole('button', { name: 'Refresh settings availability' }));
    await screen.findByText(/The current revision is 4/);
    expect(save()).toBeDisabled();
    await userEvent.click(screen.getByRole('button', { name: 'Review retained draft' }));
    expect(amount()).toHaveValue('80');
    await userEvent.click(save());
    await screen.findByText('Settings saved at revision 5.');
    expect(local.writes[1]!.settings.model_budget).toMatchObject({
      request_models: models,
      owners: retained.owners,
      default_policy: { turn_limit: 80 },
    });
  },
);

it.each([false, undefined])(
  'keeps clean policy unavailable after refreshing request availability %s',
  async (request) => {
    const local = transport(retained, true, request);
    render(<PlatformSettings client={local.client} />);
    await screen.findByLabelText('Enable spend controls');
    expect(amount()).toBeDisabled();
    expect(save()).toBeDisabled();
    await userEvent.click(screen.getByRole('button', { name: 'Refresh settings availability' }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Refresh settings availability' })).toBeEnabled(),
    );
    expect(screen.queryByText(/Your draft is retained/)).not.toBeInTheDocument();
    expect(save()).toBeDisabled();
    local.gates(true, true);
    await userEvent.click(screen.getByRole('button', { name: 'Refresh settings availability' }));
    await waitFor(() => expect(amount()).toBeEnabled());
    expect(screen.queryByText(/Your draft is retained/)).not.toBeInTheDocument();
    expect(amount()).toBeEnabled();
    expect(save()).toBeEnabled();
    expect(local.writes).toHaveLength(0);
  },
);
