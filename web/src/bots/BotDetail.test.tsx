import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ApiError } from '@chronoai/cma-sdk';
import { expect, it, vi } from 'vitest';
import { BotDetail } from './BotDetail';
import { bot, botClient } from './fixtures.test-support';
import { BotStatus } from './BotStatus';

it('shows completed setup stages only when the recorded sequential checkpoint proves them', () => {
  const view = render(
    <BotStatus
      bot={{ ...bot, state: 'creating', operation: { operation: 'create', step: 'set_callback' } }}
    />,
  );
  expect(screen.getByText('Connect Agent provider · Done')).toBeInTheDocument();
  expect(screen.getByText('Set callback · In progress')).toHaveAttribute('aria-current', 'step');
  expect(screen.getByText('Create message route · Waiting')).toBeInTheDocument();
  view.rerender(
    <BotStatus
      bot={{
        ...bot,
        state: 'creating',
        operation: { operation: 'create', step: 'future_checkpoint' },
      }}
    />,
  );
  expect(screen.queryByRole('list', { name: 'Consumer setup progress' })).not.toBeInTheDocument();
});

it('shows synthetic test evidence and waits for explicit deletion confirmation and completed cleanup', async () => {
  let value = bot;
  let removed = false;
  const result = {
    accepted_after_ms: 42,
    first_text: 'Ready to help.',
    level: 'L2',
    tested_at: bot.updated_at,
  };
  const client = botClient({
    get: vi.fn(async () => {
      if (removed) throw new ApiError(404, 'Not found');
      return value;
    }),
    test: vi.fn(async () => {
      value = { ...value, last_test: result };
      return result;
    }),
    delete: vi.fn(async () => {
      value = {
        ...value,
        state: 'deleting',
        operation: { operation: 'delete', step: 'delete_routes' },
      };
      return value;
    }),
  });
  render(<BotDetail client={client} id={bot.bot_id} onNavigate={vi.fn()} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'Run synthetic test' }));
  await screen.findByText('Accepted after 42 ms · Provider level L2');
  expect(
    screen.getByText('This synthetic test does not send a message to your channel.'),
  ).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Delete consumer' }));
  expect(screen.getByRole('button', { name: 'Confirm deletion' })).toBeDisabled();
  expect(client.bots.delete).not.toHaveBeenCalled();
  await user.click(screen.getByRole('checkbox'));
  await user.click(screen.getByRole('button', { name: 'Confirm deletion' }));
  await screen.findByText('Removing message routes');
  expect(screen.queryByText(/Consumer deleted\./)).not.toBeInTheDocument();
  removed = true;
  await user.click(screen.getByRole('button', { name: 'Refresh status' }));
  await screen.findByText(/Consumer deleted\./);
  expect(client.bots.delete).toHaveBeenCalledTimes(1);
});
it('carries the consumer state as a badge beside its title', async () => {
  let value: typeof bot = bot;
  const client = botClient({
    get: vi.fn(async () => value),
    disable: vi.fn(async () => {
      value = { ...value, state: 'disabled' };
      return value;
    }),
  });
  render(<BotDetail client={client} id={bot.bot_id} onNavigate={vi.fn()} />);
  expect(screen.getByRole('status', { name: 'Loading consumer' })).toBeInTheDocument();
  const header = (await screen.findByRole('heading', { name: bot.name })).closest('header')!;
  expect(within(header).getByText('Active').closest('.badge')).toHaveAttribute(
    'data-tone',
    'neutral',
  );
  await userEvent.setup().click(screen.getByRole('button', { name: 'Disable consumer' }));
  await waitFor(() =>
    expect(within(header).getByText('Disabled').closest('.badge')).toHaveAttribute(
      'data-tone',
      'muted',
    ),
  );
});

it('shows a recorded failure as an error alert', async () => {
  const failed = { ...bot, state: 'failed' as const, failure: 'The channel rejected the key.' };
  render(
    <BotDetail
      client={botClient({ get: vi.fn(async () => failed) })}
      id={bot.bot_id}
      onNavigate={vi.fn()}
    />,
  );
  expect(await screen.findByRole('alert')).toHaveTextContent('The channel rejected the key.');
  expect(screen.getByText('Needs attention').closest('.badge')).toHaveAttribute(
    'data-tone',
    'error',
  );
});

it('uses explicit disable, enable and rotation actions, with no automatic retry after uncertainty', async () => {
  let value = bot;
  const client = botClient({
    get: vi.fn(async () => value),
    disable: vi.fn(async () => {
      value = { ...value, state: 'disabled' };
      return value;
    }),
    enable: vi.fn(async () => {
      value = { ...value, state: 'active' };
      return value;
    }),
    rotateKeys: vi.fn().mockRejectedValue(new TypeError('connection lost')),
  });
  render(<BotDetail client={client} id={bot.bot_id} onNavigate={vi.fn()} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'Disable consumer' }));
  await user.click(await screen.findByRole('button', { name: 'Enable consumer' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Rotate keys' })).toBeEnabled());
  await user.click(screen.getByRole('button', { name: 'Rotate keys' }));
  await user.click(screen.getByRole('button', { name: 'Confirm key rotation' }));
  await screen.findByText(/key rotation outcome is unconfirmed/);
  await user.click(screen.getByRole('button', { name: 'Refresh status' }));
  expect(client.bots.rotateKeys).toHaveBeenCalledTimes(1);
  expect(screen.getByRole('button', { name: 'Rotate keys' })).toBeDisabled();
});
