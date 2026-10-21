import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ChannelEndpoints } from './ChannelEndpoints';
import { ChannelTable } from './ChannelTable';
import { EventGateway } from './EventGateway';

afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const endpoints = {
  custom_event_url: 'https://gateway.example/v1/channels/opaque/events',
  nyxid_callback_url: 'https://gateway.example/v1/callbacks/opaque',
};

function clipboard(writeText: (text: string) => Promise<void>) {
  // user-event installs the clipboard jsdom lacks.
  userEvent.setup();
  return vi.spyOn(navigator.clipboard, 'writeText').mockImplementation(writeText);
}

describe('channel endpoints', () => {
  it('lists each address with its purpose and copies it exactly', async () => {
    const write = clipboard(async () => undefined);
    render(<ChannelEndpoints endpoints={endpoints} />);
    const rows = within(screen.getByRole('region', { name: 'Endpoints' })).getAllByRole('listitem');
    expect(rows).toHaveLength(2);
    expect(rows[0]).toHaveTextContent('Event endpoint');
    expect(rows[0]).toHaveTextContent('Post custom events from your source here.');
    expect(within(rows[1]!).getByText(endpoints.nyxid_callback_url).tagName).toBe('CODE');
    fireEvent.click(screen.getByRole('button', { name: 'Copy Callback endpoint' }));
    // The status region is always present; wait for the clipboard's answer in it.
    expect(await within(rows[1]!).findByText('Copied')).toHaveAttribute('role', 'status');
    expect(write).toHaveBeenCalledWith(endpoints.nyxid_callback_url);
    expect(within(rows[0]!).getByRole('status')).toHaveTextContent('');
  });

  it('selects the address for copying by hand when the clipboard refuses', async () => {
    clipboard(async () => {
      throw new DOMException('Denied', 'NotAllowedError');
    });
    render(<ChannelEndpoints endpoints={endpoints} />);
    fireEvent.click(screen.getByRole('button', { name: 'Copy Event endpoint' }));
    expect(await screen.findByText(/Copy didn’t work/)).toBeInTheDocument();
    expect(window.getSelection()?.toString()).toBe(endpoints.custom_event_url);
  });

  it('shows the Telegram webhook only for a channel that has one', () => {
    render(
      <ChannelEndpoints
        endpoints={{ ...endpoints, telegram_webhook_url: 'https://gateway.example/v1/telegram/x' }}
      />,
    );
    expect(screen.getByRole('button', { name: 'Copy Telegram webhook' })).toBeInTheDocument();
    expect(screen.getAllByRole('listitem')).toHaveLength(3);
  });
});

describe('the channels table', () => {
  it('says there are no channels yet with what a channel is for', () => {
    render(<ChannelTable channels={[]} loading={false} onSelect={vi.fn()} />);
    const empty = screen.getByRole('cell', { name: /No channels yet/ });
    expect(empty).toHaveTextContent('Connect an event source to your agent.');
    expect(empty).toHaveAttribute('colspan', '5');
  });

  it('reserves rows while loading and says when the list could not be read', () => {
    const { rerender } = render(<ChannelTable channels={[]} loading onSelect={vi.fn()} />);
    expect(screen.getByRole('status')).toHaveTextContent('Loading channels');
    rerender(
      <ChannelTable
        channels={[]}
        loading={false}
        unavailable="Channels could not be loaded."
        onSelect={vi.fn()}
      />,
    );
    expect(screen.getByRole('cell', { name: 'Channels could not be loaded.' })).toBeVisible();
    expect(screen.queryByText('No channels yet')).not.toBeInTheDocument();
  });
});

it('returns from a channel to the table through its breadcrumb', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          items: [
            {
              record_id: 'record',
              channel_id: 'channel',
              version: 2,
              provider: { slug: 'cma_codex', level: 'l1' },
              definition: { name: 'Build assistant', sources: [] },
              endpoints,
            },
          ],
        }),
      ),
    ),
  );
  render(
    <EventGateway
      session={{
        authenticated: true,
        subject: 'owner',
        permissions: ['cma-eg:channels:read'],
        expires_at: 9999999999,
      }}
      denied={vi.fn()}
    />,
  );
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'Build assistant' }));
  const detail = screen.getByRole('region', { name: 'Build assistant details' });
  expect(within(detail).getByText(endpoints.custom_event_url)).toBeVisible();
  // Read-only access sees the channel but none of the commands it cannot use.
  expect(screen.queryByRole('button', { name: 'Edit channel' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'New channel' })).not.toBeInTheDocument();
  await user.click(within(detail).getByRole('link', { name: 'Channels' }));
  expect(screen.getByRole('table', { name: 'Your channels' })).toBeVisible();
  expect(detail).not.toBeVisible();
});
