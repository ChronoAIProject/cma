import { act, fireEvent, render, screen, within, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { EventGateway } from './EventGateway';
// Load the real graph module before exercising lifecycle and editor interactions.
import './DeliveryFlowCanvas';
import type { Channel, ChannelListItem } from './channelTypes';

const session = {
  authenticated: true as const,
  subject: 'owner',
  permissions: ['cma-eg:admin'],
  expires_at: 9999999999,
};
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status });
const channel = (ready: unknown = true, version = 7): Channel => ({
  record_id: 'original-record',
  channel_id: 'original-channel',
  version,
  owner: 'owner',
  provider: { slug: 'cma_codex', level: 'l2' },
  definition: { name: 'Original', sources: [], schema_version: 1, creation: { ready } },
  endpoints: {
    custom_event_url: 'https://example.test/events',
    nyxid_callback_url: 'https://example.test/callback',
  },
});
function fixture(initial: ChannelListItem) {
  const current = { row: initial, status: 200, recovered: initial, save: false, deleteStatus: 409 };
  const fetcher = vi.fn<typeof fetch>(async (url, init) => {
    if (init?.method === 'PUT') {
      if (!current.save) throw new TypeError('Result unknown');
      return json(channel(true, 10));
    }
    if (init?.method === 'DELETE')
      return json({ error: 'Cleanup not confirmed' }, current.deleteStatus);
    if (url === '/api/event-gateway/v1/channels')
      return json({ items: [current.row] }, current.status);
    if (url === '/api/event-gateway/v1/channel-capabilities') return json({});
    return json(current.recovered);
  });
  vi.stubGlobal('fetch', fetcher);
  vi.stubGlobal('EventSource', undefined);
  const view = render(<EventGateway session={session} denied={vi.fn()} />);
  return {
    current,
    fetcher,
    view,
    writes: () => fetcher.mock.calls.filter(([, init]) => init?.method && init.method !== 'GET'),
  };
}
async function open() {
  fireEvent.click(
    await within(screen.getByRole('tree', { name: 'Channels' })).findByRole('button', {
      name: /^Original/,
    }),
  );
  fireEvent.click(await screen.findByRole('button', { name: /^Channel: Original/ }));
  return within(screen.getByRole('dialog', { name: 'Channel settings' }));
}
async function refresh() {
  await act(async () => {
    fireEvent.click(screen.getByRole('button', { name: 'Refresh channel status' }));
  });
}
afterEach(() => vi.unstubAllGlobals());

it.each([false, null, 'true'])(
  'blocks actual edit/routing/test submissions when creation ready=%j',
  async (ready) => {
    const { writes } = fixture(channel(ready));
    const settings = await open();
    expect(settings.getByRole('button', { name: 'Save channel' })).toBeDisabled();
    expect(settings.getByRole('button', { name: 'Send test event' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: 'Add producer' })).not.toBeInTheDocument();
    await act(async () => {
      fireEvent.submit(settings.getByRole('button', { name: 'Save channel' }).closest('form')!);
      fireEvent.submit(settings.getByRole('button', { name: 'Save routing' }).closest('form')!);
      fireEvent.click(settings.getByRole('button', { name: 'Send test event' }));
    });
    expect(writes()).toHaveLength(0);
    expect(settings.getByRole('button', { name: 'Delete channel' })).toBeEnabled();
  },
);

it('keeps the mounted draft through readiness, failed read and malformed metadata then resumes explicitly', async () => {
  const { current, writes } = fixture(channel());
  const settings = await open();
  const input = settings.getByLabelText('Channel name');
  fireEvent.change(input, { target: { value: 'Retained draft' } });
  current.row = channel(false, 8);
  await refresh();
  expect(settings.getByLabelText('Channel name')).toBe(input);
  expect(input).toHaveValue('Retained draft');
  expect(input).toBeDisabled();
  await act(async () => {
    fireEvent.submit(input.closest('form')!);
  });
  current.status = 503;
  await refresh();
  expect(input).toBeDisabled();
  expect(screen.getAllByText(/status stale/).length).toBeGreaterThan(0);
  current.status = 200;
  current.row = { ...channel(true, 9), definition: { ...channel().definition, creation: null } };
  await refresh();
  expect(input).toBeDisabled();
  current.row = channel(true, 10);
  await refresh();
  expect(settings.getByLabelText('Channel name')).toBe(input);
  expect(input).toHaveValue('Retained draft');
  expect(input).toBeEnabled();
  expect(writes()).toHaveLength(0);
});

it('retains the exact uncertain request while readiness blocks retry and never replays on refresh', async () => {
  const { current, writes } = fixture(channel());
  const settings = await open();
  fireEvent.change(settings.getByLabelText('Channel name'), {
    target: { value: 'Retained request' },
  });
  await act(async () => {
    fireEvent.click(settings.getByRole('button', { name: 'Save channel' }));
  });
  const first = writes()[0];
  expect(first).toBeDefined();
  current.row = channel(false, 8);
  await refresh();
  const retry = settings.getByRole('button', { name: 'Retry same request' });
  expect(retry).toBeDisabled();
  await act(async () => {
    fireEvent.click(retry);
    fireEvent.submit(settings.getByLabelText('Channel name').closest('form')!);
  });
  expect(writes()).toHaveLength(1);
  current.row = channel(true, 9);
  await refresh();
  expect(retry).toBeEnabled();
  expect(writes()).toHaveLength(1);
  current.save = true;
  await act(async () => {
    fireEvent.click(retry);
  });
  expect(writes()).toHaveLength(2);
  expect(writes()[1]?.[0]).toBe(first?.[0]);
  expect(writes()[1]?.[1]).toMatchObject({
    method: first?.[1]?.method,
    body: first?.[1]?.body,
    headers: first?.[1]?.headers,
  });
  expect(JSON.parse(String(first?.[1]?.body))).toMatchObject({
    expected_version: 7,
    name: 'Retained request',
  });
});

it('quarantines a contradictory same-version refresh until another authoritative read', async () => {
  const { current, writes } = fixture(channel(false));
  const settings = await open();
  current.row = channel(true);
  await refresh();
  expect(settings.getByRole('button', { name: 'Save channel' })).toBeDisabled();
  expect(screen.getAllByText(/Creation status unconfirmed/).length).toBeGreaterThan(0);
  await refresh();
  expect(settings.getByRole('button', { name: 'Save channel' })).toBeEnabled();
  expect(writes()).toHaveLength(0);
});

it.each([false, true])(
  'automatic address recovery retains readiness=%s and updates row and detail together',
  async (ready) => {
    const initial = { ...channel(ready), channel_id: undefined, endpoints: undefined };
    const { current, writes } = fixture(initial);
    current.recovered = channel(ready);
    const settings = await open();
    expect(settings.getByRole('button', { name: 'Save channel' })).toHaveProperty(
      'disabled',
      !ready,
    );
    const row = within(screen.getByRole('tree', { name: 'Channels' })).getByRole('button', {
      name: /^Original/,
    });
    expect(row).toHaveTextContent(ready ? 'Setup complete' : 'Creation incomplete');
    expect(writes()).toHaveLength(0);
  },
);

it.each([403, 409, 503])(
  'retains the original row/version after delete refusal or uncertainty %s',
  async (status) => {
    const { current, writes } = fixture(channel(false));
    current.deleteStatus = status;
    const settings = await open();
    fireEvent.click(settings.getByRole('button', { name: 'Delete channel' }));
    await act(async () => {
      fireEvent.click(settings.getByRole('button', { name: 'Confirm delete' }));
    });
    expect(settings.getByRole('alert')).toBeVisible();
    expect(screen.getByRole('heading', { name: 'Original' })).toBeVisible();
    expect(writes()).toHaveLength(1);
    expect(writes()[0]?.[0]).toBe(
      '/api/event-gateway/v1/channels/original-channel?expected_version=7',
    );
    expect(settings.getByRole('button', { name: 'Save channel' })).toBeDisabled();
  },
);

it('retains a draft after an authorization read refusal but blocks all writes', async () => {
  const { current, writes } = fixture(channel());
  const settings = await open();
  const input = settings.getByLabelText('Channel name');
  fireEvent.change(input, { target: { value: 'Retained after refusal' } });
  current.status = 403;
  await refresh();
  expect(input).toBeInTheDocument();
  expect(input).toHaveValue('Retained after refusal');
  expect(settings.getByRole('button', { name: 'Save channel' })).toBeDisabled();
  expect(settings.getByRole('button', { name: 'Delete channel' })).toBeDisabled();
  expect(writes()).toHaveLength(0);
});

it('ignores an automatic address response after switching the signed-in owner', async () => {
  let release!: (value: Response) => void;
  const deferred = new Promise<Response>((resolve) => {
    release = resolve;
  });
  const initial = { ...channel(false), channel_id: undefined, endpoints: undefined };
  const { fetcher, view, writes } = fixture(initial);
  fetcher.mockImplementation(async (url) =>
    url === '/api/event-gateway/v1/channels'
      ? json({ items: [initial] })
      : url === '/api/event-gateway/v1/channel-capabilities'
        ? json({})
        : deferred,
  );
  fireEvent.click(
    await within(screen.getByRole('tree', { name: 'Channels' })).findByRole('button', {
      name: /^Original/,
    }),
  );
  await waitFor(() =>
    expect(fetcher.mock.calls.some(([url]) => String(url).includes('channel-records'))).toBe(true),
  );
  fetcher.mockImplementation(async () => json({ items: [] }));
  view.rerender(<EventGateway session={{ ...session, subject: 'other' }} denied={vi.fn()} />);
  await act(async () => {
    release(json(channel(true, 9)));
  });
  expect(screen.queryByLabelText('Channel name')).not.toBeInTheDocument();
  expect(screen.queryByRole('heading', { name: 'Original' })).not.toBeInTheDocument();
  expect(writes()).toHaveLength(0);
});

it.each([false, true])(
  'manual original-address recovery still checks readiness=%s',
  async (ready) => {
    const initial = { ...channel(ready), channel_id: undefined, endpoints: undefined };
    const { fetcher, writes } = fixture(initial);
    fetcher.mockImplementation(async (url) => {
      if (url === '/api/event-gateway/v1/channels') return json({ items: [initial] });
      if (url === '/api/event-gateway/v1/channel-capabilities') return json({});
      if (url === '/api/event-gateway/v1/channels/original-channel') return json(channel(ready));
      return json({}, 404);
    });
    fireEvent.click(
      await within(screen.getByRole('tree')).findByRole('button', { name: /^Original/ }),
    );
    const input = await screen.findByLabelText('Original channel ID');
    fireEvent.change(input, { target: { value: 'original-channel' } });
    await act(async () => {
      fireEvent.submit(input.closest('form')!);
    });
    fireEvent.click(await screen.findByRole('button', { name: /^Channel: Original/ }));
    expect(screen.getByRole('button', { name: 'Save channel' })).toHaveProperty('disabled', !ready);
    expect(writes()).toHaveLength(0);
  },
);

it.each([
  { version: 6 },
  { record_id: 'foreign-record' },
  { channel_id: 'wrong-address' },
  { owner: 'other' },
])('refuses recovered address evidence with an old or changed identity: %j', async (change) => {
  const initial = { ...channel(false), endpoints: undefined };
  const { current, writes } = fixture(initial);
  current.recovered = { ...channel(true, 8), ...change };
  fireEvent.click(
    await within(screen.getByRole('tree')).findByRole('button', { name: /^Original/ }),
  );
  await screen.findByLabelText('Original channel ID');
  expect(screen.queryByRole('button', { name: 'Save channel' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Delete channel' })).not.toBeInTheDocument();
  expect(screen.getAllByText(/Creation incomplete/).length).toBeGreaterThan(0);
  expect(writes()).toHaveLength(0);
});

it('keeps a newer incomplete version when a delayed older list arrives', async () => {
  const { current, writes } = fixture(channel(false, 9));
  const settings = await open();
  current.row = channel(true, 8);
  await refresh();
  expect(settings.getByRole('button', { name: 'Save channel' })).toBeDisabled();
  expect(screen.getAllByText(/Version 9/).some((element) => element.closest('header'))).toBe(true);
  expect(writes()).toHaveLength(0);
});

it('does not grant actions to a read-only user even with complete setup', async () => {
  const { view, writes } = fixture(channel());
  view.rerender(
    <EventGateway
      session={{ ...session, permissions: ['cma-eg:channels:read'] }}
      denied={vi.fn()}
    />,
  );
  await open();
  expect(screen.queryByRole('button', { name: 'Save channel' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Delete channel' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Send test event' })).not.toBeInTheDocument();
  expect(writes()).toHaveLength(0);
});

it('does not use a list read begun before a contradiction to clear that contradiction', async () => {
  let resolveAddress!: (value: Response) => void;
  let resolveList!: (value: Response) => void;
  const address = new Promise<Response>((resolve) => {
    resolveAddress = resolve;
  });
  const list = new Promise<Response>((resolve) => {
    resolveList = resolve;
  });
  const initial = { ...channel(false), channel_id: undefined, endpoints: undefined };
  const { fetcher, writes } = fixture(initial);
  let reads = 0;
  fetcher.mockImplementation(async (url) => {
    if (url === '/api/event-gateway/v1/channels') {
      reads++;
      return list;
    }
    if (url === '/api/event-gateway/v1/channel-capabilities') return json({});
    return address;
  });
  fireEvent.click(
    await within(screen.getByRole('tree')).findByRole('button', { name: /^Original/ }),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Refresh channel status' }));
  await waitFor(() => expect(reads).toBe(1));
  await act(async () => {
    resolveAddress(json(channel(true)));
  });
  await act(async () => {
    resolveList(json({ items: [channel(true)] }));
  });
  fireEvent.click(await screen.findByRole('button', { name: /^Channel: Original/ }));
  expect(screen.getByRole('button', { name: 'Save channel' })).toBeDisabled();
  expect(screen.getAllByText(/Creation status unconfirmed/).length).toBeGreaterThan(0);
  expect(writes()).toHaveLength(0);
});

it.each([
  ['human', 'owner', true],
  ['organization', 'owner', false],
  ['human', 'other', false],
] as const)(
  'checks the public owner kind and subject during original-address recovery: %s/%s',
  async (kind, subject, accepted) => {
    const initial = {
      ...channel(),
      channel_id: undefined,
      endpoints: undefined,
      definition: { ...channel().definition, owner: { kind: 'human', subject: 'owner' } },
    };
    const { current, writes } = fixture(initial);
    current.recovered = {
      ...channel(),
      definition: { ...channel().definition, owner: { kind, subject } },
    };
    fireEvent.click(
      await within(screen.getByRole('tree')).findByRole('button', { name: /^Original/ }),
    );
    if (accepted) {
      fireEvent.click(await screen.findByRole('button', { name: /^Channel: Original/ }));
      expect(screen.getByRole('button', { name: 'Save channel' })).toBeEnabled();
    } else {
      await screen.findByLabelText('Original channel ID');
      expect(screen.queryByRole('button', { name: 'Save channel' })).not.toBeInTheDocument();
      expect(screen.queryByRole('button', { name: 'Delete channel' })).not.toBeInTheDocument();
    }
    expect(writes()).toHaveLength(0);
  },
);
