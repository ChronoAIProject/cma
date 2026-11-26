import { useState } from 'react';
import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { EventGateway } from '../EventGateway';
import { ManagedConnectionFields } from './ManagedConnectionFields';
import { channelRequest, type Fields } from './model';
afterEach(() => vi.unstubAllGlobals());
const manual = {
  sources: [
    { type: 'nyxid_relay', platform: 'discord', verify_raw_event: true, route_ids: ['retained'] },
  ],
};
const listPath = '/api/event-gateway/v1/channel-connections';
const toggleLabel = 'Use an existing CMA-managed Telegram connection';
const ready = { connection_ref: 'ready-owned', status: 'ready', label: 'My Telegram bot' };
function response(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), { status });
}
function Editor({
  inspect,
  denied,
}: {
  inspect: (value: Fields) => void;
  denied: (status?: number) => void;
}) {
  const [value, setValue] = useState<Fields>(manual);
  return (
    <ManagedConnectionFields
      value={value}
      denied={denied}
      onChange={(next) => {
        inspect(next);
        setValue(next);
      }}
    />
  );
}
it('loads only owned connections on demand, keeps admission closed and restores the exact manual draft', async () => {
  const fetcher = vi.fn().mockImplementation(
    async () =>
      new Response(
        JSON.stringify({
          items: [
            { connection_ref: 'ready-owned', status: 'ready', label: 'My Telegram bot' },
            {
              connection_ref: 'pending-owned',
              status: 'awaiting_platform',
              label: 'Unfinished bot',
            },
          ],
          next_after: null,
        }),
      ),
  );
  vi.stubGlobal('fetch', fetcher);
  const inspect = vi.fn();
  render(<Editor inspect={inspect} denied={vi.fn()} />);
  expect(fetcher).not.toHaveBeenCalled();
  const toggle = screen.getByLabelText('Use an existing CMA-managed Telegram connection');
  fireEvent.click(toggle);
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  expect(inspect.mock.lastCall?.[0]).toMatchObject({
    sources: [],
    connection_admission: { type: 'allowlist', sender_ids: [] },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Saved Telegram connection' }));
  expect(await screen.findByRole('menuitemradio', { name: /Unfinished bot/ })).toHaveAttribute(
    'aria-disabled',
    'true',
  );
  fireEvent.click(screen.getByRole('menuitemradio', { name: 'My Telegram bot' }));
  expect(inspect.mock.lastCall?.[0]).toMatchObject({ connection_ref: 'ready-owned', sources: [] });
  fireEvent.click(toggle);
  expect(inspect.mock.lastCall?.[0]).toEqual(manual);
  fireEvent.click(toggle);
  expect(inspect.mock.lastCall?.[0]).toMatchObject({ connection_ref: 'ready-owned', sources: [] });
  expect(fetcher).toHaveBeenCalledOnce();
  expect(fetcher.mock.calls[0]?.[0]).toBe('/api/event-gateway/v1/channel-connections');
});
it('omits manual platform credentials only from the managed create request, retaining them in the draft', () => {
  const credentials = {
    agent_key: 'provider-fixture',
    channel_key: 'relay-fixture',
    telegram_bot_token: 'manual-fixture',
    telegram_webhook_secret: 'webhook-fixture',
  };
  const request = channelRequest(
    { sources: [], connection_ref: 'ready-owned' },
    credentials,
    undefined,
    'record',
  );
  expect(request.credentials).toEqual({ agent_key: 'provider-fixture' });
  expect(credentials.telegram_bot_token).toBe('manual-fixture');
  expect(() => channelRequest({ sources: [], connection_ref: '' }, credentials)).toThrow(
    /Choose a ready/,
  );
});
it.each([401, 403])(
  'routes HTTP %i through access handling without showing empty or retry options',
  async (status) => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(response({}, status)));
    const denied = vi.fn();
    render(<Editor inspect={vi.fn()} denied={denied} />);
    fireEvent.click(screen.getByLabelText(toggleLabel));
    await waitFor(() => expect(denied).toHaveBeenCalledWith(status));
    expect(screen.queryByText(/No saved CMA connections/)).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Retry connections' })).not.toBeInTheDocument();
  },
);

it.each([{ items: [] }, { items: [ready] }])(
  'checks the continuation after an empty page: $items',
  async ({ items }) => {
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(response({ items: [], next_after: 'next / page' }))
      .mockResolvedValueOnce(response({ items, next_after: null }));
    vi.stubGlobal('fetch', fetcher);
    const inspect = vi.fn();
    render(<Editor inspect={inspect} denied={vi.fn()} />);
    fireEvent.click(screen.getByLabelText(toggleLabel));
    const more = await screen.findByRole('button', { name: 'Load more connections' });
    expect(screen.queryByText(/No saved CMA connections/)).not.toBeInTheDocument();
    expect(fetcher).toHaveBeenCalledOnce();
    fireEvent.click(more);
    if (items.length === 0) {
      expect(await screen.findByText(/No saved CMA connections/)).toBeVisible();
    } else {
      fireEvent.click(screen.getByRole('button', { name: 'Saved Telegram connection' }));
      fireEvent.click(await screen.findByRole('menuitemradio', { name: 'My Telegram bot' }));
      expect(inspect.mock.lastCall?.[0]).toMatchObject({ connection_ref: ready.connection_ref });
      expect(screen.queryByText(/No saved CMA connections/)).not.toBeInTheDocument();
    }
    expect(screen.queryByRole('button', { name: 'Load more connections' })).not.toBeInTheDocument();
    expect(fetcher.mock.calls.map(([url]) => url)).toEqual([
      listPath,
      `${listPath}?after=next%20%2F%20page`,
    ]);
  },
);

it('retries only the failed page and preserves the managed selection and independent manual draft', async () => {
  const fetcher = vi
    .fn<typeof fetch>()
    .mockResolvedValueOnce(response({ items: [ready], next_after: 'second' }))
    .mockResolvedValueOnce(
      response({ code: 'provider_unavailable', detail: 'Fixture failure detail' }, 503),
    )
    .mockResolvedValueOnce(
      response({
        items: [ready, { ...ready, connection_ref: 'second', label: 'Second bot' }],
        next_after: null,
      }),
    );
  vi.stubGlobal('fetch', fetcher);
  const inspect = vi.fn();
  const denied = vi.fn();
  render(<Editor inspect={inspect} denied={denied} />);
  const toggle = screen.getByLabelText(toggleLabel);
  fireEvent.click(toggle);
  const more = await screen.findByRole('button', { name: 'Load more connections' });
  const picker = screen.getByRole('button', { name: 'Saved Telegram connection' });
  fireEvent.click(picker);
  fireEvent.click(screen.getByRole('menuitemradio', { name: 'My Telegram bot' }));
  fireEvent.click(more);
  expect(await screen.findByRole('alert')).toHaveTextContent(/draft is retained/i);
  expect(screen.queryByText('Fixture failure detail')).not.toBeInTheDocument();
  expect(screen.queryByText(/No saved CMA connections/)).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Load more connections' })).not.toBeInTheDocument();
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(denied).not.toHaveBeenCalled();

  fireEvent.click(toggle);
  expect(inspect.mock.lastCall?.[0]).toEqual(manual);
  fireEvent.click(toggle);
  expect(inspect.mock.lastCall?.[0]).toMatchObject({ connection_ref: ready.connection_ref });
  fireEvent.click(screen.getByRole('button', { name: 'Retry connections' }));
  fireEvent.click(screen.getByRole('button', { name: 'Saved Telegram connection' }));
  expect(await screen.findByRole('menuitemradio', { name: 'Second bot' })).toBeVisible();
  expect(screen.getAllByRole('menuitemradio', { name: 'My Telegram bot' })).toHaveLength(1);
  expect(screen.getByRole('menuitemradio', { name: 'My Telegram bot' })).toHaveAttribute(
    'aria-checked',
    'true',
  );
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  expect(fetcher.mock.calls.map(([url]) => url)).toEqual([
    listPath,
    `${listPath}?after=second`,
    `${listPath}?after=second`,
  ]);
});

it.each([200, 403])('discards a previous owner’s outstanding HTTP %i reply', async (status) => {
  let resolvePrevious!: (response: Response) => void;
  const previous = new Promise<Response>((resolve) => (resolvePrevious = resolve));
  const lists = vi
    .fn<typeof fetch>()
    .mockReturnValueOnce(previous)
    .mockResolvedValueOnce(
      response({ items: [{ ...ready, connection_ref: 'current', label: 'Current bot' }] }),
    );
  const fetcher = vi.fn<typeof fetch>((url, init) => {
    if (url === listPath) return lists(url, init);
    return Promise.resolve(
      response({
        platforms: [{ id: 'telegram', connection_methods: ['managed_create'] }],
        items: [],
      }),
    );
  });
  vi.stubGlobal('fetch', fetcher);
  const denied = vi.fn();
  const view = (subject: string) => (
    <EventGateway
      session={{
        authenticated: true,
        permissions: ['cma-eg:admin'],
        expires_at: 9999999999,
        subject,
      }}
      denied={denied}
    />
  );
  const openEditor = () =>
    fireEvent.click(
      within(screen.getByRole('region', { name: 'Channel workspace' })).getByRole('button', {
        name: 'New channel',
      }),
    );
  const { rerender } = await act(async () => render(view('previous-owner')));
  openEditor();
  fireEvent.click(screen.getByLabelText(toggleLabel));
  expect(lists).toHaveBeenCalledOnce();
  const signal = lists.mock.calls[0]?.[1]?.signal;
  expect(signal?.aborted).toBe(false);

  await act(async () => rerender(view('current-owner')));
  expect(signal?.aborted).toBe(true);
  openEditor();
  fireEvent.click(screen.getByLabelText(toggleLabel));
  fireEvent.click(screen.getByRole('button', { name: 'Saved Telegram connection' }));
  expect(await screen.findByRole('menuitemradio', { name: 'Current bot' })).toBeVisible();
  await act(async () =>
    resolvePrevious(response({ items: [ready], next_after: 'old-page' }, status)),
  );
  expect(screen.getByRole('menuitemradio', { name: 'Current bot' })).toBeVisible();
  expect(screen.queryByRole('menuitemradio', { name: 'My Telegram bot' })).not.toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Load more connections' })).not.toBeInTheDocument();
  expect(denied).not.toHaveBeenCalled();
});

it.each([200, 403])(
  'discards an outstanding HTTP %i page when the editor is cancelled',
  async (status) => {
    let resolveCancelled!: (response: Response) => void;
    const cancelled = new Promise<Response>((resolve) => (resolveCancelled = resolve));
    const lists = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(response({ items: [ready], next_after: 'second' }))
      .mockReturnValueOnce(cancelled)
      .mockResolvedValue(response({ items: [], next_after: null }));
    vi.stubGlobal(
      'fetch',
      vi.fn<typeof fetch>((url, init) =>
        typeof url === 'string' && url.startsWith(listPath)
          ? lists(url, init)
          : Promise.resolve(
              response({
                platforms: [{ id: 'telegram', connection_methods: ['managed_create'] }],
                items: [],
              }),
            ),
      ),
    );
    const denied = vi.fn();
    await act(async () =>
      render(
        <EventGateway
          session={{
            authenticated: true,
            permissions: ['cma-eg:admin'],
            expires_at: 9999999999,
            subject: 'fixture-owner',
          }}
          denied={denied}
        />,
      ),
    );
    const openEditor = () =>
      fireEvent.click(
        within(screen.getByRole('region', { name: 'Channel workspace' })).getByRole('button', {
          name: 'New channel',
        }),
      );
    openEditor();
    fireEvent.change(screen.getByLabelText('Channel name'), {
      target: { value: 'Retained draft' },
    });
    fireEvent.click(screen.getByLabelText(toggleLabel));
    const more = await screen.findByRole('button', { name: 'Load more connections' });
    fireEvent.click(screen.getByRole('button', { name: 'Saved Telegram connection' }));
    fireEvent.click(screen.getByRole('menuitemradio', { name: 'My Telegram bot' }));
    fireEvent.click(more);
    expect(lists).toHaveBeenCalledTimes(2);
    expect(lists.mock.calls[1]?.[0]).toBe(`${listPath}?after=second`);
    const signal = lists.mock.calls[1]?.[1]?.signal;
    expect(signal?.aborted).toBe(false);

    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(screen.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();
    expect.soft(signal?.aborted).toBe(true);
    await act(async () =>
      resolveCancelled(
        response(
          { items: [{ ...ready, connection_ref: 'cancelled-page', label: 'Cancelled bot' }] },
          status,
        ),
      ),
    );
    expect.soft(denied).not.toHaveBeenCalled();

    openEditor();
    expect(screen.getByLabelText('Channel name')).toHaveValue('Retained draft');
    fireEvent.click(screen.getByRole('button', { name: 'Saved Telegram connection' }));
    expect
      .soft(screen.queryByRole('menuitemradio', { name: 'My Telegram bot' }))
      .toHaveAttribute('aria-checked', 'true');
    expect(screen.queryByRole('menuitemradio', { name: 'Cancelled bot' })).not.toBeInTheDocument();
  },
);

it.each([
  { page: 'first', hide: 'cancel', status: 200 },
  { page: 'first', hide: 'cancel', status: 403 },
  { page: 'continuation', hide: 'cancel', status: 200 },
  { page: 'continuation', hide: 'cancel', status: 403 },
  { page: 'continuation', hide: 'portal', status: 403 },
])(
  'keeps the fresh $page read when $hide ended a pending HTTP $status read',
  async ({ page, hide, status }) => {
    let finishOld!: (response: Response) => void;
    let finishFresh!: (response: Response) => void;
    const old = new Promise<Response>((resolve) => (finishOld = resolve));
    const fresh = new Promise<Response>((resolve) => (finishFresh = resolve));
    const continuation = page === 'continuation';
    const cursor = 'next / page';
    const lists = vi.fn<typeof fetch>();
    if (continuation) {
      lists.mockResolvedValueOnce(response({ items: [ready], next_after: cursor }));
    }
    lists.mockReturnValueOnce(old).mockReturnValueOnce(fresh);
    const fetcher = vi.fn<typeof fetch>((url, init) =>
      typeof url === 'string' && url.startsWith(listPath)
        ? lists(url, init)
        : Promise.resolve(
            response({
              platforms: [{ id: 'telegram', connection_methods: ['managed_create'] }],
              items: [],
            }),
          ),
    );
    vi.stubGlobal('fetch', fetcher);
    const denied = vi.fn();
    const view = (active: boolean) => (
      <EventGateway
        active={active}
        session={{
          authenticated: true,
          permissions: ['cma-eg:admin'],
          expires_at: 9999999999,
          subject: 'fixture-owner',
        }}
        denied={denied}
      />
    );
    const openEditor = () =>
      fireEvent.click(
        within(screen.getByRole('region', { name: 'Channel workspace' })).getByRole('button', {
          name: 'New channel',
        }),
      );
    const picker = () => screen.getByRole('button', { name: 'Saved Telegram connection' });
    const { rerender } = await act(async () => render(view(true)));
    openEditor();
    fireEvent.change(screen.getByLabelText('Channel name'), {
      target: { value: 'Retained draft' },
    });
    fireEvent.change(screen.getByLabelText('Route IDs 1'), { target: { value: 'retained-route' } });
    fireEvent.click(screen.getByLabelText(toggleLabel));
    if (continuation) {
      const more = await screen.findByRole('button', { name: 'Load more connections' });
      fireEvent.click(picker());
      fireEvent.click(screen.getByRole('menuitemradio', { name: 'My Telegram bot' }));
      fireEvent.click(more);
    }
    const oldSignal = lists.mock.lastCall?.[1]?.signal;
    expect(oldSignal?.aborted).toBe(false);
    if (hide === 'portal') {
      rerender(view(false));
    } else {
      fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    }
    expect(oldSignal?.aborted).toBe(true);
    expect(screen.queryByRole('button', { name: 'Cancel' })).not.toBeInTheDocument();

    if (hide === 'portal') {
      rerender(view(true));
    } else {
      openEditor();
    }
    expect(screen.getByLabelText(toggleLabel)).toBeChecked();
    expect(screen.getByLabelText('Channel name')).toHaveValue('Retained draft');
    expect(screen.queryByText('Loading your connections…')).not.toBeInTheDocument();
    expect(screen.queryByText(/No saved CMA connections/)).not.toBeInTheDocument();
    fireEvent.click(
      screen.getByRole('button', {
        name: continuation ? 'Load more connections' : 'Load connections',
      }),
    );
    const path = continuation ? `${listPath}?after=next%20%2F%20page` : listPath;
    expect(lists.mock.calls.map(([url]) => url)).toEqual(
      continuation ? [listPath, path, path] : [path, path],
    );
    const freshSignal = lists.mock.lastCall?.[1]?.signal;
    await act(async () =>
      finishOld(
        response(
          {
            items: [{ ...ready, connection_ref: 'old', label: 'Cancelled bot' }],
            next_after: 'old',
          },
          status,
        ),
      ),
    );
    expect(denied).not.toHaveBeenCalled();
    expect(freshSignal?.aborted).toBe(false);
    expect(screen.getByText('Loading your connections…')).toBeVisible();
    expect(screen.getByLabelText('Channel name')).toHaveValue('Retained draft');
    fireEvent.click(picker());
    expect(screen.queryByRole('menuitemradio', { name: 'Cancelled bot' })).not.toBeInTheDocument();
    if (continuation) {
      expect(screen.getByRole('menuitemradio', { name: 'My Telegram bot' })).toHaveAttribute(
        'aria-checked',
        'true',
      );
    }
    await act(async () =>
      finishFresh(
        response({
          items: [{ ...ready, connection_ref: 'fresh', label: 'Fresh bot' }],
          next_after: null,
        }),
      ),
    );
    expect(screen.getAllByRole('menuitemradio', { name: 'Fresh bot' })).toHaveLength(1);
    expect(screen.queryByText('Loading your connections…')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Load more connections' })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole('menuitemradio', { name: 'Fresh bot' }));
    fireEvent.click(screen.getByLabelText(toggleLabel));
    expect(screen.getByLabelText('Route IDs 1')).toHaveValue('retained-route');
    expect(fetcher.mock.calls.every(([, init]) => (init?.method ?? 'GET') === 'GET')).toBe(true);
  },
);
