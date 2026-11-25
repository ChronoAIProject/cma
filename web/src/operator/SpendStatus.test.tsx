import { cleanup, render, screen } from '@testing-library/react';
import { ApiError, type PlatformSpend } from '@cma/client';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import { SpendStatus } from './SpendStatus';
import { operatorClient } from './testFixtures';

const open: PlatformSpend = {
  metering_enabled: true,
  settings_revision: 4,
  levels: { warning: 800, stop: 1000, disposable: 900 },
  committed: 120,
  ledger: {
    period_start: '2026-11-01T00:00:00Z',
    period_end: '2026-12-01T00:00:00Z',
    settings_revision: 4,
    spent: 100,
    reserved: 20,
    adjustments: 0,
    cloud: {
      observed_through: '2026-11-20T00:00:00Z',
      units: 40,
      recorded_at: '2026-11-20T00:05:00Z',
      settings_revision: 4,
    },
    closures: [],
  },
};

const closed: PlatformSpend = {
  ...open,
  ledger: {
    ...open.ledger!,
    cloud: undefined,
    closures: [
      {
        reason: 'metering_unavailable',
        since: '2026-11-21T00:00:00Z',
        settings_revision: 4,
        committed_at_entry: 120,
      },
      {
        reason: 'spend_warning',
        since: '2026-11-19T00:00:00Z',
        settings_revision: 4,
        committed_at_entry: 800,
      },
    ],
  },
};

async function check(spend: PlatformSpend) {
  const client = operatorClient();
  client.admin.readSpend.mockResolvedValueOnce(spend);
  const user = userEvent.setup();
  render(<SpendStatus client={client} disabled={false} />);
  await user.click(screen.getByRole('button', { name: 'Check platform spend' }));
  return { client, user };
}

it('distinguishes metering that is not enabled from an unavailable enabled period', async () => {
  const { client } = await check({ metering_enabled: false, settings_revision: 1 });
  await screen.findByText(/Platform spend metering is not enabled/);
  expect(screen.queryByText(/Metering unavailable/)).not.toBeInTheDocument();
  expect(
    screen.queryByRole('button', { name: 'Record cloud observation' }),
  ).not.toBeInTheDocument();
  expect(client.admin.recordSpendObservation).not.toHaveBeenCalled();
});

it('shows an open period with its levels, commitment and latest cloud observation', async () => {
  await check(open);
  await screen.findByText('Platform spend is open. New spend commitments are admitted.');
  expect(screen.getByText('Warning level').nextSibling).toHaveTextContent('800');
  expect(screen.getByText('Stop level').nextSibling).toHaveTextContent('1000');
  expect(screen.getByText('Disposable level').nextSibling).toHaveTextContent('900');
  expect(screen.getByText('Committed').nextSibling).toHaveTextContent('120');
  expect(screen.getByText('Latest cloud observation').nextSibling).toHaveTextContent(
    /40 units through/,
  );
});

it('lists every closure with its start and reports a missing cloud observation', async () => {
  await check(closed);
  await screen.findByText('Platform spend is closed to new commitments.');
  expect(screen.getByText(/Metering unavailable since/)).toBeInTheDocument();
  expect(screen.getByText(/Spend warning since/)).toBeInTheDocument();
  expect(screen.getByText('None recorded in this period')).toBeInTheDocument();
});

it('validates and records a cloud observation, then shows the returned state', async () => {
  const { client, user } = await check(open);
  await screen.findByRole('button', { name: 'Record cloud observation' });
  await user.type(screen.getByLabelText('Observed through (RFC 3339)'), '2026-11-21');
  await user.type(screen.getByLabelText('Cloud units'), '4.5');
  await user.click(screen.getByRole('button', { name: 'Record cloud observation' }));
  expect(screen.getByText(/Enter an RFC 3339 time/)).toBeInTheDocument();
  expect(screen.getByText('Enter a whole number of accounting units.')).toBeInTheDocument();
  expect(client.admin.recordSpendObservation).not.toHaveBeenCalled();

  client.admin.recordSpendObservation.mockResolvedValueOnce(closed);
  await user.clear(screen.getByLabelText('Observed through (RFC 3339)'));
  await user.type(screen.getByLabelText('Observed through (RFC 3339)'), '2026-11-21T00:00:00Z');
  await user.clear(screen.getByLabelText('Cloud units'));
  await user.type(screen.getByLabelText('Cloud units'), '55');
  await user.click(screen.getByRole('button', { name: 'Record cloud observation' }));
  await screen.findByText('Cloud observation recorded.');
  expect(client.admin.recordSpendObservation).toHaveBeenCalledWith({
    source: 'cloud',
    observed_through: '2026-11-21T00:00:00Z',
    units: 55,
  });
  expect(screen.getByText('Platform spend is closed to new commitments.')).toBeInTheDocument();
  expect(screen.getByLabelText('Cloud units')).toHaveValue('');
});

it('explains a refused observation and keeps the entry without retrying', async () => {
  const { client, user } = await check(open);
  client.admin.recordSpendObservation.mockRejectedValueOnce(
    new ApiError(422, 'Invalid', 'invalid_request'),
  );
  await user.type(
    await screen.findByLabelText('Observed through (RFC 3339)'),
    '2026-11-21T00:00:00Z',
  );
  await user.type(screen.getByLabelText('Cloud units'), '55');
  await user.click(screen.getByRole('button', { name: 'Record cloud observation' }));
  await screen.findByText(/a different observation for the same time may already exist/);
  expect(client.admin.recordSpendObservation).toHaveBeenCalledTimes(1);
  expect(screen.getByLabelText('Cloud units')).toHaveValue('55');
  expect(
    screen.getByText('Platform spend is open. New spend commitments are admitted.'),
  ).toBeInTheDocument();
});

it('reports unavailable writes and unavailable platform spend as retained entries', async () => {
  for (const code of ['model_budget_writes_unavailable', 'platform_spend_unavailable']) {
    const { client, user } = await check(open);
    client.admin.recordSpendObservation.mockRejectedValueOnce(
      new ApiError(503, 'Unavailable', code),
    );
    await user.type(
      await screen.findByLabelText('Observed through (RFC 3339)'),
      '2026-11-21T00:00:00Z',
    );
    await user.type(screen.getByLabelText('Cloud units'), '7');
    await user.click(screen.getByRole('button', { name: 'Record cloud observation' }));
    await screen.findByText(/Your entry has been retained/);
    expect(screen.getByText(code)).toBeInTheDocument();
    expect(screen.getByLabelText('Cloud units')).toHaveValue('7');
    expect(client.admin.recordSpendObservation).toHaveBeenCalledTimes(1);
    cleanup();
  }
});
