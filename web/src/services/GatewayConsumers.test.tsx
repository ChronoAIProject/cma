import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { bot, botClient } from '../bots/fixtures.test-support';
import { GatewayConsumers } from './GatewayConsumers';

afterEach(() => vi.restoreAllMocks());
it('lists authoritative consumer names, creation time and creator without polling', async () => {
  const interval = vi.spyOn(window, 'setInterval');
  const client = botClient();
  const navigate = vi.fn();
  render(<GatewayConsumers client={client} selected={bot.bot_id} onNavigate={navigate} />);
  await act(async () => {});
  const row = screen.getByRole('button', { name: /Research assistant/ });
  expect(row).toHaveAttribute('aria-current', 'page');
  expect(row.querySelector('time')).toHaveAttribute('datetime', bot.created_at);
  expect(within(row).getByText('By owner')).toBeVisible();
  await userEvent.click(row);
  expect(navigate).toHaveBeenCalledWith({ kind: 'bot', botId: bot.bot_id });
  await userEvent.click(screen.getByRole('button', { name: 'New consumer' }));
  expect(navigate).toHaveBeenCalledWith({ kind: 'bot_new' });
  expect(client.bots.list).toHaveBeenCalledOnce();
  expect(interval).not.toHaveBeenCalled();
});
it('retains rows during failed pagination and retries the authoritative first page explicitly', async () => {
  const second = { ...bot, bot_id: 'second', name: 'Second consumer' };
  const list = vi
    .fn()
    .mockResolvedValueOnce({ items: [bot], next_after: 'next' })
    .mockRejectedValueOnce(new Error('Connection unavailable'))
    .mockResolvedValueOnce({ items: [bot, second], next_after: null });
  const client = botClient({ list });
  render(<GatewayConsumers client={client} onNavigate={vi.fn()} />);
  await screen.findByRole('button', { name: /Research assistant/ });
  await userEvent.click(screen.getByRole('button', { name: 'Load more consumers' }));
  expect(await screen.findByText('Connection unavailable')).toBeVisible();
  expect(screen.getByRole('button', { name: /Research assistant/ })).toBeVisible();
  expect(list.mock.calls[1]?.[0]).toMatchObject({ after: 'next', limit: 50 });
  await userEvent.click(screen.getByRole('button', { name: 'Try again' }));
  expect(await screen.findByRole('button', { name: /Second consumer/ })).toBeVisible();
  expect(list.mock.calls[2]?.[0]).toMatchObject({ after: undefined, limit: 50 });
});
it('aborts its public list read when the account or portal unmounts', async () => {
  let finish: (value: { items: (typeof bot)[]; next_after: null }) => void = () => {};
  const list = vi.fn(
    () =>
      new Promise<{ items: (typeof bot)[]; next_after: null }>((resolve) => {
        finish = resolve;
      }),
  );
  const client = botClient({ list });
  const { unmount } = render(<GatewayConsumers client={client} onNavigate={vi.fn()} />);
  const signal = (list.mock.calls[0] as unknown as [{ signal: AbortSignal }])[0].signal;
  unmount();
  expect(signal.aborted).toBe(true);
  await act(async () => finish({ items: [bot], next_after: null }));
  expect(screen.queryByText('Research assistant')).not.toBeInTheDocument();
});
