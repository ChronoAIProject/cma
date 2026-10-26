import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { Channel } from './ChannelEditor';
import { ChannelOutboundMessage } from './ChannelOutboundMessage';
import { outboundStorageKey } from './outboundJournal';
import { RelayCapabilitiesContext, type RelayDiscovery } from './RelayCapabilities';
const channel = {
  channel_id: 'channel',
  record_id: 'record',
  version: 1,
  definition: {
    sources: [{ type: 'nyxid_relay', platform: 'telegram', route_ids: ['saved-route'] }],
  },
} as unknown as Channel;
const capabilities = {
  status: 'ready',
  authoring: { outbound_messages_writes_enabled: true },
} as RelayDiscovery;
const receipt = (status = 'prepared', delivery: unknown = null) => ({
  delivery_id: 'delivery',
  route_id: 'saved-route',
  status,
  delivery,
});
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status });
afterEach(() => vi.unstubAllGlobals());
function view(owner = 'alice', current = channel) {
  return render(
    <RelayCapabilitiesContext value={capabilities}>
      <ChannelOutboundMessage
        owner={owner}
        channel={current}
        selection={{ kind: 'producer', index: 0 }}
        denied={vi.fn()}
      />
    </RelayCapabilitiesContext>,
  );
}
it('requires preparation and explicit send and shows the saved destination', async () => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(json(receipt(), 201))
    .mockResolvedValueOnce(json(receipt('committed', { outcome: 'confirmed' })));
  vi.stubGlobal('fetch', fetch);
  const mounted = view();
  fireEvent.change(screen.getByLabelText('Message text'), { target: { value: 'hello' } });
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  await screen.findByRole('button', { name: 'Send message' });
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(screen.getByText('Destination: saved-route')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  await screen.findByText('The platform accepted the message.');
  expect(fetch).toHaveBeenCalledTimes(2);
  mounted.unmount();
  view();
  expect(screen.getByText('The platform accepted the message.')).toBeVisible();
  expect(fetch).toHaveBeenCalledTimes(2);
});
it('retains ambiguous commits and offers status checks without a resend action', async () => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(json(receipt(), 201))
    .mockRejectedValueOnce(new TypeError('lost'))
    .mockResolvedValueOnce(json(receipt('committed', { outcome: 'unknown' })));
  vi.stubGlobal('fetch', fetch);
  const mounted = view();
  fireEvent.change(screen.getByLabelText('Message text'), { target: { value: 'hello' } });
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Send message' }));
  await screen.findByText(/Sending was not confirmed/);
  mounted.unmount();
  view();
  expect(fetch).toHaveBeenCalledTimes(2);
  expect(screen.getByText('hello')).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Send message' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Check delivery status' }));
  await waitFor(() => expect(fetch).toHaveBeenCalledTimes(3));
  expect(fetch.mock.calls[2]?.[1]?.method).toBe('GET');
});
it('does not offer initiated sends without capability confirmation', () => {
  render(
    <ChannelOutboundMessage
      owner="alice"
      channel={channel}
      selection={{ kind: 'producer', index: 0 }}
      denied={vi.fn()}
    />,
  );
  expect(screen.queryByRole('button', { name: 'Review message' })).not.toBeInTheDocument();
});

it('recovers an uncertain preparation after remount with its exact original key and body', async () => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockRejectedValueOnce(new TypeError('lost'))
    .mockResolvedValueOnce(json(receipt(), 201));
  vi.stubGlobal('fetch', fetch);
  const mounted = view();
  fireEvent.change(screen.getByLabelText('Message text'), { target: { value: 'retained text' } });
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  await screen.findByText('lost');
  mounted.unmount();
  view();
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(screen.getByText('retained text')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Check saved request' }));
  await screen.findByRole('button', { name: 'Send message' });
  expect(fetch.mock.calls[1]?.[1]?.headers).toEqual(fetch.mock.calls[0]?.[1]?.headers);
  expect(fetch.mock.calls[1]?.[1]?.body).toEqual(fetch.mock.calls[0]?.[1]?.body);
});
it('isolates account and channel state and restores only the original owner slot', async () => {
  const fetch = vi.fn<typeof globalThis.fetch>().mockRejectedValue(new TypeError('lost'));
  vi.stubGlobal('fetch', fetch);
  const mounted = view();
  fireEvent.change(screen.getByLabelText('Message text'), { target: { value: 'private draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  await screen.findByText('lost');
  mounted.unmount();
  const otherOwner = view('bob');
  expect(screen.queryByText('private draft')).not.toBeInTheDocument();
  expect(screen.getByLabelText('Message text')).toHaveValue('');
  otherOwner.unmount();
  const otherChannel = view('alice', {
    ...channel,
    record_id: 'other-record',
    channel_id: 'other',
  });
  expect(screen.queryByText('private draft')).not.toBeInTheDocument();
  otherChannel.unmount();
  view();
  expect(screen.getByText('private draft')).toBeVisible();
  expect(fetch).toHaveBeenCalledTimes(1);
});
it('refuses network effects when the request cannot be persisted', () => {
  const fetch = vi.fn<typeof globalThis.fetch>();
  vi.stubGlobal('fetch', fetch);
  view();
  vi.stubGlobal('sessionStorage', {
    setItem() {
      throw new Error('full');
    },
  });
  fireEvent.change(screen.getByLabelText('Message text'), { target: { value: 'hello' } });
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  expect(screen.getByText(/The message could not be saved/)).toBeVisible();
  expect(fetch).not.toHaveBeenCalled();
});
it('journals commit uncertainty before dispatch, including a reload while send is in flight', async () => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(json(receipt(), 201))
    .mockImplementationOnce(async () => {
      const saved = JSON.parse(sessionStorage.getItem(outboundStorageKey('alice', 'record'))!);
      expect(saved.intent.receipt.delivery_id).toBe('delivery');
      expect(saved.intent.uncertain).toBe(true);
      return await new Promise<Response>(() => {});
    });
  vi.stubGlobal('fetch', fetch);
  const mounted = view();
  fireEvent.change(screen.getByLabelText('Message text'), { target: { value: 'hello' } });
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Send message' }));
  await waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
  mounted.unmount();
  view();
  expect(screen.queryByRole('button', { name: 'Send message' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Check delivery status' })).toBeEnabled();
  expect(fetch).toHaveBeenCalledTimes(2);
});
it('does not discard unreadable storage or offer a fresh send', () => {
  sessionStorage.setItem(outboundStorageKey('alice', 'record'), '{broken');
  const fetch = vi.fn<typeof globalThis.fetch>();
  vi.stubGlobal('fetch', fetch);
  view();
  expect(screen.getByText(/saved message could not be read/)).toBeVisible();
  expect(screen.getByRole('button', { name: 'Review message' })).toBeDisabled();
  expect(fetch).not.toHaveBeenCalled();
});
