import { render, screen } from '@testing-library/react';
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
  expect(screen.getByText('Provider level L2')).toBeInTheDocument();
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
});
