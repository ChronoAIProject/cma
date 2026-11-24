import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { RelayCapabilitiesContext } from '../RelayCapabilities';
import { ChannelEditor } from '../ChannelEditor';

afterEach(() => vi.unstubAllGlobals());

it('focuses a blank channel name before other required fields and submits a trimmed name', async () => {
  const fetcher = vi.fn().mockResolvedValue(new Response('{}'));
  vi.stubGlobal('fetch', fetcher);
  render(<ChannelEditor ownerSubject="local-owner" onDone={vi.fn()} denied={vi.fn()} />);
  const name = screen.getByLabelText('Channel name');
  const create = screen.getByRole('button', { name: 'Create channel' });
  fireEvent.click(create);
  expect(name).toHaveFocus();
  fireEvent.change(name, { target: { value: '      ' } });
  fireEvent.click(create);
  expect(name).toHaveFocus();
  expect(name).toBeInvalid();
  expect(name).toHaveAccessibleDescription(/Enter a channel name/);
  expect(
    within(screen.getByRole('group', { name: 'Review' })).getByText('Name required'),
  ).toBeVisible();
  expect(fetcher).not.toHaveBeenCalled();

  for (const [label, value] of [
    ['Channel name', '  Relay updates  '],
    ['NyxID issuer', 'https://example.test'],
    ['Signing key ID', 'relay-key'],
    ['Route IDs 1', 'relay-route'],
    ['NyxID channel key', 'fixture-channel-key'],
    ['Provider agent key', 'fixture-agent-key'],
  ] as const) {
    fireEvent.change(screen.getByLabelText(label), { target: { value } });
  }
  fireEvent.click(create);
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  expect(JSON.parse(fetcher.mock.calls[0]![1].body).name).toBe('Relay updates');
});

it('removes an added producer and keeps the surviving draft and its alternate connection settings', () => {
  render(<ChannelEditor ownerSubject="local-owner" onDone={vi.fn()} denied={vi.fn()} />);
  fireEvent.change(screen.getByLabelText('Channel name'), { target: { value: 'Team channel' } });
  fireEvent.change(screen.getByLabelText('NyxID channel key'), {
    target: { value: 'fixture-key' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Add producer' }));
  fireEvent.click(screen.getByRole('button', { name: 'Add producer' }));
  expect(screen.queryByRole('button', { name: 'Remove producer 1' })).not.toBeInTheDocument();
  const second = within(screen.getByRole('group', { name: 'Producer 2' }));
  const third = within(screen.getByRole('group', { name: 'Producer 3' }));
  fireEvent.change(second.getByLabelText('Prompt template'), { target: { value: 'Remove me' } });
  fireEvent.change(third.getByLabelText('Prompt template'), { target: { value: 'Keep me' } });
  fireEvent.click(third.getByRole('button', { name: 'Producer 3 connection type' }));
  fireEvent.click(screen.getByRole('menuitemradio', { name: 'Direct Telegram bot' }));
  fireEvent.change(third.getByLabelText('Bot ID'), { target: { value: '123' } });

  fireEvent.click(second.getByRole('button', { name: 'Remove producer 2' }));
  expect(screen.queryByRole('group', { name: 'Producer 3' })).not.toBeInTheDocument();
  expect(screen.getByRole('group', { name: 'Producer 1' })).toHaveTextContent(
    'NyxID messaging connection',
  );
  expect(second.getByLabelText('Bot ID')).toHaveValue('123');
  expect(screen.getByLabelText('Channel name')).toHaveValue('Team channel');
  expect(screen.getByLabelText('NyxID channel key')).toHaveValue('fixture-key');
  fireEvent.click(second.getByRole('button', { name: 'Producer 2 connection type' }));
  fireEvent.click(screen.getByRole('menuitemradio', { name: 'Custom event' }));
  expect(second.getByLabelText('Prompt template')).toHaveValue('Keep me');
});

it('keeps required Agent identity visible while optional expert controls stay collapsed', () => {
  const fetcher = vi.fn();
  vi.stubGlobal('fetch', fetcher);
  render(<ChannelEditor onDone={vi.fn()} denied={vi.fn()} />);
  const owner = screen.getByLabelText('Owner subject');
  expect(owner.closest('details')).toHaveAttribute('open');
  expect(owner).toHaveAccessibleDescription('Required.');
  expect(screen.getByText('Advanced model connection').closest('details')).not.toHaveAttribute(
    'open',
  );
  expect(screen.getByLabelText('Service scope 1')).toBeVisible();
  expect(screen.getByLabelText('Permission mode')).toBeVisible();
  const access = within(screen.getByRole('group', { name: 'Access & reply policy' }));
  expect(access.getByRole('button', { name: 'Admission policy' })).toHaveTextContent(
    'All messages',
  );
  expect(access.getByRole('button', { name: 'Group incoming messages by' })).toHaveTextContent(
    'Each sender in each conversation',
  );
  expect(fetcher).not.toHaveBeenCalled();
});

it('reveals a newly added required expert field and preserves the draft across disclosure changes', () => {
  render(<ChannelEditor ownerSubject="local-owner" onDone={vi.fn()} denied={vi.fn()} />);
  const details = screen.getByText('Advanced Agent tools and environment').closest('details')!;
  fireEvent.click(screen.getByText('Advanced Agent tools and environment'));
  fireEvent.click(screen.getByRole('button', { name: 'Add skill' }));
  const skill = screen.getByLabelText('Skill 1 ID');
  expect(skill).toBeRequired();
  expect(details).toHaveAttribute('open');
  fireEvent.change(skill, { target: { value: 'local-skill' } });
  fireEvent.click(screen.getByText('Advanced Agent tools and environment'));
  expect(details).not.toHaveAttribute('open');
  fireEvent.change(screen.getByLabelText('Channel name'), {
    target: { value: 'Retained channel' },
  });
  fireEvent.click(screen.getByText('Advanced Agent tools and environment'));
  expect(screen.getByLabelText('Skill 1 ID')).toHaveValue('local-skill');
  expect(
    within(screen.getByRole('group', { name: 'Review' })).getByText('Retained channel'),
  ).toBeVisible();
});

it('retains HMAC credential editing for managed connections with webhook replies', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(JSON.stringify({ items: [] }))));
  render(
    <RelayCapabilitiesContext value={{ status: 'ready', managedTelegramSetup: true }}>
      <ChannelEditor ownerSubject="local-owner" onDone={vi.fn()} denied={vi.fn()} />
    </RelayCapabilitiesContext>,
  );
  fireEvent.click(screen.getByLabelText('Use an existing CMA-managed Telegram connection'));
  await screen.findByText(/No saved CMA connections/);
  fireEvent.click(screen.getByRole('button', { name: 'Destination 1 type' }));
  fireEvent.click(screen.getByRole('menuitemradio', { name: 'Result webhook' }));
  expect(screen.getByRole('button', { name: 'Add HMAC credential' })).toBeVisible();
  expect(screen.queryByLabelText('Replace HMAC credentials')).not.toBeInTheDocument();
  expect(screen.getByLabelText('Provider agent key')).toBeRequired();
  expect(screen.queryByLabelText('Telegram bot token')).not.toBeInTheDocument();
  expect(screen.queryByLabelText('NyxID channel key')).not.toBeInTheDocument();
});

it.each(['loading', 'legacy', 'failed', 'ready'] as const)(
  'shows Telegram relay guidance without offering managed setup (%s)',
  (status) => {
    const fetcher = vi.fn();
    vi.stubGlobal('fetch', fetcher);
    render(
      <RelayCapabilitiesContext value={{ status }}>
        <ChannelEditor ownerSubject="local-owner" onDone={vi.fn()} denied={vi.fn()} />
      </RelayCapabilitiesContext>,
    );
    const producer = screen.getByRole('button', { name: 'Producer 1 connection type' });
    fireEvent.click(producer);
    expect(
      screen.queryByRole('menuitemradio', { name: /managed Telegram/i }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('menuitemradio', { name: 'Direct Telegram bot' }));
    fireEvent.click(producer);
    fireEvent.click(screen.getByRole('menuitemradio', { name: 'NyxID messaging connection' }));
    expect(
      screen.getByText(
        'Connect your Telegram bot in NyxID, then select a relay route for that bot.',
      ),
    ).toBeVisible();
    expect(
      screen.queryByLabelText('Use an existing CMA-managed Telegram connection'),
    ).not.toBeInTheDocument();
    expect(
      screen.queryByRole('button', { name: 'Saved Telegram connection' }),
    ).not.toBeInTheDocument();
    expect(fetcher).not.toHaveBeenCalled();
  },
);

it('selects a saved Telegram connection in Connection setup and retains it after capability refresh', async () => {
  const fetcher = vi.fn().mockResolvedValue(
    new Response(
      JSON.stringify({
        items: [{ connection_ref: 'ready-owned', status: 'ready', label: 'My Telegram bot' }],
      }),
    ),
  );
  vi.stubGlobal('fetch', fetcher);
  const view = (enabled: boolean) => (
    <RelayCapabilitiesContext value={{ status: 'ready', managedTelegramSetup: enabled }}>
      <ChannelEditor ownerSubject="local-owner" onDone={vi.fn()} denied={vi.fn()} />
    </RelayCapabilitiesContext>
  );
  const mounted = render(view(true));
  const setup = within(screen.getByRole('group', { name: 'Connection setup' }));
  const toggle = setup.getByLabelText('Use an existing CMA-managed Telegram connection');
  expect(fetcher).not.toHaveBeenCalled();
  fireEvent.click(toggle);
  const picker = setup.getByRole('button', { name: 'Saved Telegram connection' });
  fireEvent.click(picker);
  fireEvent.click(await screen.findByRole('menuitemradio', { name: 'My Telegram bot' }));
  expect(picker).toHaveTextContent('My Telegram bot');
  mounted.rerender(view(false));
  expect(setup.getByLabelText('Use an existing CMA-managed Telegram connection')).toBeChecked();
  expect(setup.getByRole('button', { name: 'Saved Telegram connection' })).toHaveTextContent(
    'My Telegram bot',
  );
  expect(fetcher.mock.calls.map(([url]) => url)).toEqual([
    '/api/event-gateway/v1/channel-connections',
  ]);
});
