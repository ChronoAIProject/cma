import { act, render, screen, waitFor } from '@testing-library/react';
import { beforeAll, expect, it, vi } from 'vitest';
import { CmaClientProvider } from './api/context';
import { unavailableClient, type CmaWebClient } from './api/client';
import type { User } from './auth/types';
import { BotManagement, canManageBots } from './BotManagement';
import { bot, botClient } from './bots/fixtures.test-support';

beforeAll(async () => {
  // Compile the lazy route during setup; the test measures viewer isolation.
  await import('./bots/BotPage');
});

const owner: User = {
  id: 'owner',
  handle: 'owner',
  visitor: false,
  capabilities: ['manage_own_bots'],
};
function Entry({ user = owner, client }: { user?: User; client: CmaWebClient }) {
  return (
    <CmaClientProvider client={client}>
      <BotManagement user={user} route={{ kind: 'bots' }} onNavigate={vi.fn()} />
    </CmaClientProvider>
  );
}
it('requires Bot authority and a bound client for both navigation and direct routes', () => {
  const bots = botClient();
  const client = { ...unavailableClient, botViews: bots };
  expect(canManageBots(owner, client)).toBe(true);
  for (const user of [
    { ...owner, capabilities: [] },
    { ...owner, visitor: true },
  ]) {
    expect(canManageBots(user, client)).toBe(false);
    const view = render(<Entry user={user} client={client} />);
    expect(screen.getByText('Bot management unavailable')).toBeInTheDocument();
    view.unmount();
  }
  expect(canManageBots(owner, unavailableClient)).toBe(false);
  render(<Entry client={unavailableClient} />);
  expect(screen.getByText('Bot management unavailable')).toBeInTheDocument();
  expect(bots.bots.list).not.toHaveBeenCalled();
});
it('clears protected records on viewer and client change and ignores late previous reads', async () => {
  let finish: ((value: { items: (typeof bot)[]; next_after: null }) => void) | undefined;
  const old = botClient({
    list: vi.fn(
      () =>
        new Promise<{ items: (typeof bot)[]; next_after: null }>((resolve) => {
          finish = resolve;
        }),
    ),
  });
  const first = { ...unavailableClient, botViews: old };
  const view = render(<Entry client={first} />);
  await waitFor(() => expect(old.bots.list).toHaveBeenCalled());
  const next = botClient({ list: vi.fn(async () => ({ items: [], next_after: null })) });
  const second = { ...unavailableClient, botViews: next };
  view.rerender(<Entry user={{ ...owner, id: 'other' }} client={second} />);
  await screen.findByText('No consumers yet');
  await act(async () => finish?.({ items: [bot], next_after: null }));
  expect(screen.queryByText(bot.name)).not.toBeInTheDocument();
  view.rerender(
    <Entry user={{ ...owner, id: 'other' }} client={{ ...second, botViews: botClient() }} />,
  );
  await screen.findByText(bot.name);
  view.rerender(<Entry user={{ ...owner, id: 'third' }} client={second} />);
  expect(screen.queryByText(bot.name)).not.toBeInTheDocument();
  await screen.findByText('No consumers yet');
});
