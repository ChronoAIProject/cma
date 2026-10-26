import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ApiError } from '@chronoai/cma-sdk';
import { expect, it, vi } from 'vitest';
import { BotsList } from './BotsList';
import { bot, botClient } from './fixtures.test-support';

it('lists states and provider levels, pages explicitly, and opens canonical details', async () => {
  const list = vi
    .fn()
    .mockResolvedValueOnce({ items: [bot], next_after: 'cursor-two' })
    .mockResolvedValue({ items: [], next_after: null });
  const client = botClient({ list });
  const onNavigate = vi.fn();
  render(<BotsList client={client} onNavigate={onNavigate} />);
  expect(screen.queryByText('No consumers on this page.')).not.toBeInTheDocument();
  const user = userEvent.setup();
  await user.click(await screen.findByRole('link', { name: 'Research assistant' }));
  expect(onNavigate).toHaveBeenCalledWith({ kind: 'bot', botId: bot.bot_id });
  const table = screen.getByRole('table', { name: 'Your consumers' });
  expect(
    within(table)
      .getAllByRole('columnheader')
      .map((header) => header.textContent),
  ).toEqual(['Name', 'State', 'Provider level', 'Last test', 'Updated']);
  const cells = within(within(table).getAllByRole('row')[1]!).getAllByRole('cell');
  expect(cells.map((cell) => cell.textContent?.trim()).slice(0, 4)).toEqual([
    'Research assistant',
    'Active',
    'L2',
    'Not tested',
  ]);
  expect(within(cells[4]!).getByText(/./).tagName).toBe('TIME');
  expect(within(cells[4]!).getByText(/./)).toHaveAttribute('datetime', bot.updated_at);
  await user.click(screen.getByRole('button', { name: 'Next page' }));
  await screen.findByText('No consumers on this page.');
  expect(list).toHaveBeenLastCalledWith(expect.objectContaining({ after: 'cursor-two' }));
  expect(screen.getByRole('button', { name: 'Previous page' })).toBeEnabled();
});

it('retains rows after a temporary outage but clears them after access denial', async () => {
  const list = vi
    .fn()
    .mockResolvedValueOnce({ items: [bot], next_after: null })
    .mockRejectedValueOnce(new ApiError(503, 'The Bot store is unavailable.'))
    .mockRejectedValue(new ApiError(403, 'Reconnect your CMA account.'));
  render(<BotsList client={botClient({ list })} onNavigate={vi.fn()} />);
  await screen.findByRole('link', { name: 'Research assistant' });
  const user = userEvent.setup();
  await user.click(screen.getByRole('button', { name: 'Refresh consumers' }));
  await screen.findByText('The Bot store is unavailable.');
  expect(screen.getByRole('link', { name: 'Research assistant' })).toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Refresh consumers' }));
  await screen.findByText('Reconnect your CMA account.');
  expect(screen.queryByRole('link', { name: 'Research assistant' })).not.toBeInTheDocument();
  expect(screen.getByRole('alert')).toHaveTextContent('Reconnect your CMA account.');
  // A denial is not an empty list: it must not claim there are no consumers.
  expect(screen.queryByText('No consumers yet')).not.toBeInTheDocument();
  expect(screen.queryByRole('status', { name: /Loading/ })).not.toBeInTheDocument();
});

it('draws every state as its badge tone and the last test in words', async () => {
  const states = [
    ['creating', 'Creating', 'active'],
    ['active', 'Active', 'neutral'],
    ['disabled', 'Disabled', 'muted'],
    ['failed', 'Needs attention', 'error'],
    ['deleting', 'Deleting', 'active'],
    ['deleted', 'Deleted', 'muted'],
  ] as const;
  const items = states.map(([state], index) => ({
    ...bot,
    bot_id: `bot_${state}`,
    name: `Consumer ${String(index)}`,
    state,
    channel: index === 0 ? null : bot.channel,
    last_test:
      index === 1
        ? { accepted_after_ms: 840, first_text: 'Ok', level: 'L2', tested_at: bot.updated_at }
        : null,
  }));
  const list = vi.fn().mockResolvedValue({ items, next_after: null });
  render(<BotsList client={botClient({ list })} onNavigate={vi.fn()} />);
  await screen.findByRole('link', { name: 'Consumer 0' });
  const rows = within(screen.getByRole('table')).getAllByRole('row').slice(1);
  rows.forEach((row, index) => {
    const [, word, tone] = states[index]!;
    const badge = within(row).getByText(word).closest('.badge');
    expect(badge).toHaveAttribute('data-tone', tone);
  });
  expect(rows[0]).toHaveTextContent('Not reported');
  expect(rows[1]).toHaveTextContent('Accepted after 840 ms');
});

it('reserves rows while loading, then says when there are no consumers yet', async () => {
  let finish!: (page: { items: never[]; next_after: null }) => void;
  const list = vi.fn(
    () =>
      new Promise<{ items: never[]; next_after: null }>((resolve) => {
        finish = resolve;
      }),
  );
  render(<BotsList client={botClient({ list })} onNavigate={vi.fn()} />);
  expect(screen.getByRole('status')).toHaveTextContent('Loading consumers');
  finish({ items: [], next_after: null });
  expect(await screen.findByRole('cell', { name: 'No consumers yet' })).toBeVisible();
  expect(screen.queryByRole('status')).not.toBeInTheDocument();
});
