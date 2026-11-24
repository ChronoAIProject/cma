import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor, type Channel } from './ChannelEditor';
import { ChannelOutboundMessage } from './ChannelOutboundMessage';
import { NodeInspector } from './NodeInspector';
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
const capabilities: RelayDiscovery = {
  status: 'ready',
  authoring: {
    outbound_messages_writes_enabled: true,
    dynamic_sources_writes_enabled: false,
    relay_raw_verification_writes_enabled: false,
    multi_consumer_writes_enabled: false,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [{ id: 'telegram', label: 'Telegram', verification_modes: [] }],
  },
};
const receipt = (status = 'prepared', delivery: unknown = null) => ({
  delivery_id: 'delivery',
  route_id: 'saved-route',
  status,
  delivery,
});
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status });
afterEach(() => {
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
});
function view(owner = 'alice', current = channel, denied = vi.fn(), discovery = capabilities) {
  return render(
    <RelayCapabilitiesContext value={discovery}>
      <ChannelOutboundMessage
        owner={owner}
        channel={current}
        selection={{ kind: 'producer', index: 0 }}
        denied={denied}
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
  expect(screen.getByRole('button', { name: 'Saved destination' })).toHaveTextContent(
    'saved-route',
  );
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

it.each([{ routeIds: undefined }, { routeIds: ['', '  '] }])(
  'offers route configuration when no saved destination exists ($routeIds)',
  ({ routeIds }) => {
    const fetcher = vi.fn();
    vi.stubGlobal('fetch', fetcher);
    const current: Channel = {
      ...channel,
      provider: { slug: 'cma_codex', level: 'l2' },
      definition: {
        name: 'Relay channel',
        sources: [{ type: 'nyxid_relay', platform: 'telegram', route_ids: routeIds }],
      },
      endpoints: { custom_event_url: '', nyxid_callback_url: 'https://example.test/relay' },
    };
    const selection = { kind: 'producer', index: 0 } as const;
    render(
      <RelayCapabilitiesContext value={capabilities}>
        <NodeInspector
          channel={current}
          id="relay-inspector"
          selection={selection}
          send={
            <ChannelOutboundMessage
              owner="alice"
              channel={current}
              selection={selection}
              denied={vi.fn()}
            />
          }
        >
          <ChannelEditor channel={current} section={selection} onDone={vi.fn()} denied={vi.fn()} />
        </NodeInspector>
      </RelayCapabilitiesContext>,
    );
    expect(screen.getByText(/No saved destinations/)).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Saved destination' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Review message' })).not.toBeInTheDocument();
    expect(screen.queryByRole('menu')).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Configure routes' }));
    expect(screen.getByRole('group', { name: 'NyxID connection' })).toHaveFocus();
    expect(screen.getByRole('button', { name: 'Add Route IDs' })).toBeVisible();
    expect(fetcher).not.toHaveBeenCalled();
  },
);

it('shows the QA provider error through send, two status reads and remount with no extra sends', async () => {
  const failed = {
    ...receipt('committed', { outcome: 'unknown', reason: 'provider_failure' }),
    rendering: { outcome: 'rendered' },
    expires_at: 2000000000,
  };
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(json(receipt(), 201))
    .mockImplementation(async () => json(failed));
  vi.stubGlobal('fetch', fetch);
  const mounted = view();
  fireEvent.change(screen.getByLabelText('Message text'), {
    target: { value: 'Synthetic message' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Send message' }));
  await screen.findByText(/The provider reported an error/);
  expect(screen.getByRole('status')).toHaveTextContent(
    'some or all of the message may have been sent',
  );
  for (const count of [3, 4]) {
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Check delivery status' })).toBeEnabled(),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Check delivery status' }));
    await waitFor(() => expect(fetch).toHaveBeenCalledTimes(count));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Check delivery status' })).toBeEnabled(),
    );
    expect(screen.getByRole('status')).toHaveTextContent('The provider reported an error');
  }
  const stored = sessionStorage.getItem(outboundStorageKey('alice', 'record'));
  expect(JSON.parse(stored!).intent.receipt).toEqual(failed);
  mounted.unmount();
  view();
  expect(screen.getByRole('status')).toHaveTextContent('The provider reported an error');
  expect(
    screen.queryByRole('button', { name: 'Start a separate message' }),
  ).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'New message' })).not.toBeInTheDocument();
  expect(fetch.mock.calls.map((call) => call[1]?.method)).toEqual(['POST', 'POST', 'GET', 'GET']);
  expect(sessionStorage.getItem(outboundStorageKey('alice', 'record'))).toBe(stored);
});

it('requires terminal confirmation after authoritative preparation expiry, including remount and status read', async () => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(json(receipt(), 201))
    .mockResolvedValueOnce(json({ detail: 'outbound_preparation_expired' }, 410))
    .mockResolvedValueOnce(json(receipt()))
    .mockResolvedValueOnce(json(receipt('cancelled')));
  vi.stubGlobal('fetch', fetch);
  const mounted = view();
  fireEvent.change(screen.getByLabelText('Message text'), { target: { value: 'Expired draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Send message' }));
  await screen.findByText(/This preparation can no longer be sent/);
  mounted.unmount();
  view();
  expect(screen.queryByRole('button', { name: 'New message' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Check delivery status' }));
  await waitFor(() => expect(fetch).toHaveBeenCalledTimes(3));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Cancel' })).toBeEnabled());
  expect(screen.queryByRole('button', { name: 'Send message' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  await screen.findByRole('button', { name: 'New message' });
  expect(fetch.mock.calls.map((call) => call[1]?.method)).toEqual([
    'POST',
    'POST',
    'GET',
    'DELETE',
  ]);
});

it.each([
  { ...receipt(), rendering: { outcome: 'unsupported' } },
  { ...receipt(), expires_at: 1 },
])('blocks sending and keeps cancellation for an unusable preparation: %j', async (prepared) => {
  const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValueOnce(json(prepared, 201));
  vi.stubGlobal('fetch', fetch);
  view();
  fireEvent.change(screen.getByLabelText('Message text'), { target: { value: 'Draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  await screen.findByRole('button', { name: 'Cancel' });
  expect(screen.queryByRole('button', { name: 'Send message' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'New message' })).not.toBeInTheDocument();
  expect(fetch).toHaveBeenCalledTimes(1);
});

it.each([
  { ...receipt('committed'), delivery: { outcome: 'unsupported' } },
  { ...receipt('committed'), delivery: { outcome: 'rate_limited', retry_after: 10 } },
  { ...receipt('committed'), delivery_id: 'different-delivery' },
  { ...receipt('committed'), route_id: 'different-route' },
  { ...receipt('committed'), status: 'delivered' },
])('retains the exact request when a send response cannot be verified: %j', async (response) => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(json(receipt(), 201))
    .mockResolvedValueOnce(json(response));
  vi.stubGlobal('fetch', fetch);
  view();
  fireEvent.change(screen.getByLabelText('Message text'), { target: { value: 'Original' } });
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Send message' }));
  await screen.findByText(/Sending was not confirmed/);
  expect(screen.queryByRole('button', { name: 'Send message' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'New message' })).not.toBeInTheDocument();
  const saved = JSON.parse(sessionStorage.getItem(outboundStorageKey('alice', 'record'))!);
  expect(saved.intent.draft.body).toEqual(JSON.parse(fetch.mock.calls[0]![1]!.body as string));
  expect(saved.intent.receipt).toEqual(receipt());
  expect(saved.intent.uncertain).toBe(true);
});

it('retains the intent and reports 401 during status recovery with writes disabled', async () => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(json(receipt(), 201))
    .mockResolvedValueOnce(json({ detail: 'Sign in again' }, 401));
  vi.stubGlobal('fetch', fetch);
  const mounted = view();
  fireEvent.change(screen.getByLabelText('Message text'), { target: { value: 'Saved draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  await screen.findByRole('button', { name: 'Send message' });
  mounted.unmount();
  const denied = vi.fn();
  view('alice', channel, denied, {
    status: 'ready',
    authoring: { outbound_messages_writes_enabled: false },
  } as RelayDiscovery);
  fireEvent.click(screen.getByRole('button', { name: 'Check delivery status' }));
  await waitFor(() => expect(denied).toHaveBeenCalledWith(401));
  expect(screen.getByText('Saved draft')).toBeVisible();
  expect(fetch.mock.calls[1]![1]!.method).toBe('GET');
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

it.each([
  ['retained', ['saved-route']],
  ['removed', []],
] as const)(
  'recovers an uncertain preparation after remount with its exact original key and body when routes are %s',
  async (_label, routeIds) => {
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
    view('alice', {
      ...channel,
      definition: {
        ...channel.definition,
        sources: [{ type: 'nyxid_relay', platform: 'telegram', route_ids: routeIds }],
      },
    });
    expect(fetch).toHaveBeenCalledTimes(1);
    expect(screen.getByText('retained text')).toBeVisible();
    fireEvent.click(screen.getByRole('button', { name: 'Check saved request' }));
    await screen.findByRole('button', { name: 'Send message' });
    expect(fetch.mock.calls[1]?.[1]?.headers).toEqual(fetch.mock.calls[0]?.[1]?.headers);
    expect(fetch.mock.calls[1]?.[1]?.body).toEqual(fetch.mock.calls[0]?.[1]?.body);
  },
);
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

it('allows editing after a definite first preparation body-size rejection', async () => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(json({ detail: 'Message exceeds request size limit' }, 413))
    .mockResolvedValueOnce(json(receipt(), 201));
  vi.stubGlobal('fetch', fetch);
  view();
  fireEvent.change(screen.getByLabelText('Message text'), { target: { value: 'Large draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  await screen.findByText('Message exceeds request size limit');
  expect(sessionStorage.getItem(outboundStorageKey('alice', 'record'))).toBeNull();
  expect(screen.getByLabelText('Message text')).toHaveValue('Large draft');
  fireEvent.change(screen.getByLabelText('Message text'), { target: { value: 'Shorter draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  await screen.findByRole('button', { name: 'Send message' });
  expect(JSON.parse(fetch.mock.calls[1]![1]!.body as string).message.text).toBe('Shorter draft');
  expect(fetch.mock.calls[1]![1]!.headers).not.toEqual(fetch.mock.calls[0]![1]!.headers);
  expect(fetch.mock.calls.every((call) => !String(call[0]).endsWith('/send'))).toBe(true);
});

it('retains an earlier uncertain preparation if a later recovery returns 413', async () => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockRejectedValueOnce(new TypeError('lost preparation response'))
    .mockResolvedValueOnce(json({ detail: 'Recovery request exceeds size limit' }, 413));
  vi.stubGlobal('fetch', fetch);
  const mounted = view();
  fireEvent.change(screen.getByLabelText('Message text'), { target: { value: 'Original draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  await screen.findByText('lost preparation response');
  const original = sessionStorage.getItem(outboundStorageKey('alice', 'record'));
  mounted.unmount();
  const recovered = view();
  fireEvent.click(screen.getByRole('button', { name: 'Check saved request' }));
  await screen.findByText('Recovery request exceeds size limit');
  expect(fetch.mock.calls[1]![1]!.body).toBe(fetch.mock.calls[0]![1]!.body);
  expect(fetch.mock.calls[1]![1]!.headers).toEqual(fetch.mock.calls[0]![1]!.headers);
  expect(sessionStorage.getItem(outboundStorageKey('alice', 'record'))).toBe(original);
  recovered.unmount();
  view();
  expect(screen.getByText('Original draft')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Check saved request' })).toBeEnabled();
  expect(screen.queryByLabelText('Message text')).not.toBeInTheDocument();
  expect(fetch).toHaveBeenCalledTimes(2);
});
