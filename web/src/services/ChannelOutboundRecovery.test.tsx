import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { Channel } from './ChannelEditor';
import { ChannelOutboundMessage } from './ChannelOutboundMessage';
import {
  outboundRecoveryKey,
  outboundStorageKey,
  storeOutboundIntent,
  type OutboundIntent,
} from './outboundJournal';
import { RelayCapabilitiesContext, type RelayDiscovery } from './RelayCapabilities';

const channel = {
  channel_id: 'channel',
  record_id: 'record',
  version: 1,
  definition: { sources: [{ type: 'nyxid_relay', platform: 'telegram', route_ids: ['route'] }] },
} as unknown as Channel;
const intent: OutboundIntent = {
  draft: {
    key: 'original-key',
    body: { source_id: 'relay_telegram', route_id: 'route', message: { text: 'Old draft' } },
  },
  receipt: {
    delivery_id: 'delivery',
    route_id: 'route',
    status: 'committed',
    rendering: { outcome: 'rendered' },
    delivery: { outcome: 'unknown', reason: 'provider_failure' },
  },
  uncertain: true,
};
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status });
function view(owner = 'alice', record = 'record', enabled = true, denied = vi.fn()) {
  return render(
    <RelayCapabilitiesContext
      value={
        {
          status: 'ready',
          authoring: { outbound_messages_writes_enabled: enabled },
        } as RelayDiscovery
      }
    >
      <ChannelOutboundMessage
        owner={owner}
        channel={{ ...channel, record_id: record }}
        selection={{ kind: 'producer', index: 0 }}
        denied={denied}
      />
    </RelayCapabilitiesContext>,
  );
}
function saved() {
  const original = sessionStorage;
  vi.stubGlobal('sessionStorage', {
    getItem: original.getItem.bind(original),
    setItem: original.setItem.bind(original),
    removeItem: original.removeItem.bind(original),
    clear: original.clear.bind(original),
  });
  vi.stubEnv('VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED', 'true');
  storeOutboundIntent(outboundStorageKey('alice', 'record'), intent);
}
function separate() {
  fireEvent.click(screen.getByRole('checkbox', { name: /I understand/ }));
  fireEvent.click(screen.getByRole('button', { name: 'Start a separate message' }));
}
afterEach(() => {
  vi.unstubAllGlobals();
  vi.unstubAllEnvs();
  vi.restoreAllMocks();
});

it('requires acknowledgement and a fresh review and send, retaining the old receipt across reloads', async () => {
  saved();
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(
      json({ delivery_id: 'new-delivery', route_id: 'route', status: 'prepared' }, 201),
    )
    .mockResolvedValueOnce(
      json({
        delivery_id: 'new-delivery',
        route_id: 'route',
        status: 'committed',
        delivery: { outcome: 'confirmed' },
      }),
    );
  vi.stubGlobal('fetch', fetch);
  const mounted = view();
  expect(screen.getByRole('button', { name: 'Start a separate message' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Start a separate message' }));
  expect(screen.queryByLabelText('Message text')).not.toBeInTheDocument();
  separate();
  expect(screen.getByLabelText('Message text')).toHaveValue('');
  expect(screen.getByRole('region', { name: 'Previous unconfirmed delivery' })).toHaveTextContent(
    'Old draft',
  );
  expect(fetch).not.toHaveBeenCalled();
  const archived = sessionStorage.getItem(outboundRecoveryKey('alice', 'record'));
  expect(JSON.parse(archived!).intent).toEqual(intent);
  mounted.unmount();
  view();
  expect(screen.getByLabelText('Message text')).toHaveValue('');
  expect(screen.getByRole('region', { name: 'Previous unconfirmed delivery' })).toHaveTextContent(
    'provider reported an error',
  );
  expect(fetch).not.toHaveBeenCalled();
  fireEvent.change(screen.getByLabelText('Message text'), {
    target: { value: 'Separate message' },
  });
  expect(fetch).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  await screen.findByRole('button', { name: 'Send message' });
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(JSON.parse(fetch.mock.calls[0]![1]!.body as string).message.text).toBe('Separate message');
  expect(fetch.mock.calls[0]![1]!.headers).not.toMatchObject({ 'Idempotency-Key': 'original-key' });
  expect(fetch.mock.calls[0]![1]!.headers).toMatchObject({ 'Idempotency-Key': expect.any(String) });
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  await screen.findByText('The platform accepted the message.');
  expect(String(fetch.mock.calls[1]![0]).endsWith('/new-delivery/send')).toBe(true);
  expect(JSON.parse(fetch.mock.calls[1]![1]!.body as string)).toEqual({ text: 'Separate message' });
  expect(sessionStorage.getItem(outboundRecoveryKey('alice', 'record'))).toBe(archived);
});

it('isolates previous receipts across owners and channel records and keeps reads with both gates off', async () => {
  saved();
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockImplementation(async () => json(intent.receipt));
  vi.stubGlobal('fetch', fetch);
  const mounted = view();
  separate();
  mounted.unmount();
  for (const [owner, record] of [
    ['bob', 'record'],
    ['alice', 'other-record'],
  ]) {
    const other = view(owner, record);
    expect(
      screen.queryByRole('region', { name: 'Previous unconfirmed delivery' }),
    ).not.toBeInTheDocument();
    expect(screen.queryByText('Old draft')).not.toBeInTheDocument();
    other.unmount();
  }
  vi.stubEnv('VITE_OUTBOUND_SEPARATE_MESSAGE_ENABLED', 'false');
  view('alice', 'record', false);
  expect(screen.queryByLabelText('Message text')).not.toBeInTheDocument();
  expect(
    screen.queryByRole('button', { name: 'Start a separate message' }),
  ).not.toBeInTheDocument();
  for (const count of [1, 2]) {
    fireEvent.click(screen.getByRole('button', { name: 'Check previous delivery status' }));
    await waitFor(() => expect(fetch).toHaveBeenCalledTimes(count));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Check previous delivery status' })).toBeEnabled(),
    );
  }
  expect(
    fetch.mock.calls.every(
      (call) => call[1]?.method === 'GET' && String(call[0]).endsWith('/delivery'),
    ),
  ).toBe(true);
});

it('refuses another archive while the bounded slot is occupied and preserves both receipts', async () => {
  saved();
  const previous = {
    ...intent,
    draft: { ...intent.draft, key: 'older-key' },
    receipt: { ...intent.receipt!, delivery_id: 'older-delivery' },
  };
  storeOutboundIntent(outboundRecoveryKey('alice', 'record'), previous);
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockImplementation(async () => json(intent.receipt));
  vi.stubGlobal('fetch', fetch);
  view();
  separate();
  expect(screen.getByText(/A previous delivery is still retained/)).toBeVisible();
  expect(screen.queryByLabelText('Message text')).not.toBeInTheDocument();
  expect(
    JSON.parse(sessionStorage.getItem(outboundRecoveryKey('alice', 'record'))!).intent,
  ).toEqual(previous);
  expect(JSON.parse(sessionStorage.getItem(outboundStorageKey('alice', 'record'))!).intent).toEqual(
    intent,
  );
  expect(fetch).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Check delivery status' }));
  await waitFor(() => expect(fetch).toHaveBeenCalledOnce());
  expect(fetch.mock.calls[0]![1]!.method).toBe('GET');
});

it('keeps status checks available when archive storage is full', async () => {
  saved();
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockImplementation(async () => json(intent.receipt));
  vi.stubGlobal('fetch', fetch);
  view();
  vi.spyOn(sessionStorage, 'setItem').mockImplementation(() => {
    throw new Error('full');
  });
  separate();
  expect(screen.getByText(/previous delivery could not be saved/)).toBeVisible();
  expect(screen.queryByLabelText('Message text')).not.toBeInTheDocument();
  expect(fetch).not.toHaveBeenCalled();
  for (const count of [1, 2]) {
    fireEvent.click(screen.getByRole('button', { name: 'Check delivery status' }));
    await waitFor(() => expect(fetch).toHaveBeenCalledTimes(count));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Check delivery status' })).toBeEnabled(),
    );
  }
  expect(fetch.mock.calls.every((call) => call[1]?.method === 'GET')).toBe(true);
});

it.each([401, 200])(
  'preserves a previous receipt after an unauthorized or malformed status read (%s)',
  async (status) => {
    saved();
    const fetch = vi
      .fn<typeof globalThis.fetch>()
      .mockResolvedValueOnce(
        json({ ...intent.receipt, delivery_id: 'other', detail: 'private sentinel' }, status),
      );
    vi.stubGlobal('fetch', fetch);
    const denied = vi.fn();
    view('alice', 'record', true, denied);
    separate();
    const retained = sessionStorage.getItem(outboundRecoveryKey('alice', 'record'));
    fireEvent.click(screen.getByRole('button', { name: 'Check previous delivery status' }));
    await screen.findByText(/previous delivery status could not be verified/);
    expect(screen.queryByText(/private sentinel/)).not.toBeInTheDocument();
    expect(sessionStorage.getItem(outboundRecoveryKey('alice', 'record'))).toBe(retained);
    if (status === 401) expect(denied).toHaveBeenCalledWith(401);
  },
);

it('allows dismissal only once the previous delivery is confirmed by GET', async () => {
  saved();
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(json({ ...intent.receipt, delivery: { outcome: 'confirmed' } }));
  vi.stubGlobal('fetch', fetch);
  view();
  separate();
  expect(
    screen.queryByRole('button', { name: 'Dismiss previous delivery' }),
  ).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Check previous delivery status' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Dismiss previous delivery' }));
  expect(
    screen.queryByRole('region', { name: 'Previous unconfirmed delivery' }),
  ).not.toBeInTheDocument();
  expect(sessionStorage.getItem(outboundRecoveryKey('alice', 'record'))).toBeNull();
  expect(fetch).toHaveBeenCalledOnce();
  expect(fetch.mock.calls[0]![1]!.method).toBe('GET');
});

it('renders only safe local reason text for an unrecognized provider reason', () => {
  saved();
  storeOutboundIntent(outboundStorageKey('alice', 'record'), {
    ...intent,
    receipt: {
      ...intent.receipt!,
      delivery: { outcome: 'unknown', reason: '<html>private sentinel</html>' },
    },
  });
  view();
  expect(screen.getByRole('status')).toHaveTextContent('could not be verified');
  expect(within(document.body).queryByText(/private sentinel/)).not.toBeInTheDocument();
  expect(document.body.innerHTML).not.toContain('private sentinel');
  expect(
    screen.queryByRole('button', { name: 'Start a separate message' }),
  ).not.toBeInTheDocument();
});
