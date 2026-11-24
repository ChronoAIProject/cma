import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { EventGateway } from './EventGateway';
import { ChannelEditor, type Channel } from './ChannelEditor';

afterEach(() => vi.unstubAllGlobals());
it('keeps same-name channel rows concise and independently selectable with the keyboard', async () => {
  const owner = '00000000-0000-4000-8000-000000000383';
  const created_at = '2026-11-07T00:30:12.123456789Z';
  const items = ['record-01', 'record-02'].map((record_id, index) => ({
    record_id,
    version: 1,
    provider: { slug: 'cma', level: 'L3' },
    definition: { name: 'Updates', sources: [] },
    created_at,
    owner: index === 0 ? owner : { subject: owner, display_name: '' },
  }));
  const fetcher = vi
    .fn<typeof fetch>()
    .mockImplementation(async (url) =>
      url === '/api/event-gateway/v1/channels'
        ? new Response(JSON.stringify({ items }))
        : new Response('{}', { status: 404 }),
    );
  vi.stubGlobal('fetch', fetcher);
  render(
    <EventGateway
      session={{ authenticated: true, subject: 'owner', permissions: [], expires_at: 9999999999 }}
      denied={vi.fn()}
    />,
  );
  const tree = screen.getByRole('tree', { name: 'Channels' });
  const rows = await within(tree).findAllByRole('treeitem');
  const date = new Date(created_at).toLocaleDateString([], { month: 'short', day: 'numeric' });
  for (const [index, row] of rows.entries()) {
    const label = `${items[index]!.record_id} · Updates`;
    expect(row).toHaveAccessibleName(`${label}, Created ${date}`);
    expect(within(row).getByRole('button')).toHaveAccessibleName(`${label}, Created ${date}`);
    expect(within(row).getByText(`${label}`)).toBeVisible();
    expect(row).not.toHaveAttribute('aria-label');
    expect(row.querySelector('.sr-only')).toHaveTextContent(`, Created ${date}`);
    expect(row.querySelector('time')).toHaveTextContent(date);
    expect(row.querySelector('time')).toHaveAttribute('datetime', created_at);
    for (const title of row.querySelectorAll('[title]')) {
      expect(title.getAttribute('title')).not.toContain(owner);
      expect(title.getAttribute('title')).not.toContain(created_at);
    }
  }
  rows[0]!.focus();
  await userEvent.keyboard('{ArrowDown}');
  expect(rows[1]).toHaveFocus();
  await userEvent.keyboard('{Enter}');
  expect(rows[1]).toHaveAttribute('aria-selected', 'true');
  expect(within(rows[1]!).getByRole('button')).toHaveAttribute('aria-current', 'page');
  expect(
    await screen.findByRole('link', { name: 'Open existing channel console' }),
  ).toHaveAttribute('href', '/event-gateway/legacy-console/channels/record-02');
  await userEvent.keyboard('{ArrowUp}{Enter}');
  expect(rows[0]).toHaveFocus();
  expect(rows[0]).toHaveAttribute('aria-selected', 'true');
  expect(rows[1]).toHaveAttribute('aria-selected', 'false');
  expect(
    await screen.findByRole('link', { name: 'Open existing channel console' }),
  ).toHaveAttribute('href', '/event-gateway/legacy-console/channels/record-01');
  expect(fetcher.mock.calls.map(([url]) => url)).toEqual([
    '/api/event-gateway/v1/channels',
    '/api/event-gateway/v1/channel-capabilities',
    '/api/event-gateway/v1/channel-records/record-02',
    '/api/event-gateway/v1/channel-records/record-01',
  ]);
});

it.each([undefined, { type: 'existing_agent' as const, agent_id: 'agt_borrowed' }])(
  'preserves a channel provider target when saving an unrelated name change: %j',
  async (target) => {
    const channel: Channel = {
      record_id: 'record',
      channel_id: 'channel',
      version: 7,
      provider: { slug: 'cma_codex', level: 'l2' },
      definition: {
        schema_version: 1,
        name: 'Original name',
        sources: [],
        provider: { target },
      },
      endpoints: { custom_event_url: '', nyxid_callback_url: '' },
    };
    const fetcher = vi.fn().mockResolvedValue(new Response('{}'));
    vi.stubGlobal('fetch', fetcher);
    const done = vi.fn();
    render(<ChannelEditor channel={channel} denied={vi.fn()} onDone={done} />);
    const input = screen.getByLabelText('Channel name');
    fireEvent.change(input, { target: { value: 'Renamed channel' } });
    await userEvent.click(screen.getByRole('button', { name: 'Save channel' }));
    await waitFor(() => expect(done).toHaveBeenCalledOnce());
    expect(fetcher).toHaveBeenCalledOnce();
    const [url, request] = fetcher.mock.calls[0]!;
    expect(url).toBe('/api/event-gateway/v1/channels/channel');
    expect(request.method).toBe('PUT');
    const body = JSON.parse(request.body);
    expect(body.name).toBe('Renamed channel');
    expect(body.expected_version).toBe(7);
    expect(body.provider).toEqual({ slug: 'cma_codex', ...(target ? { target } : {}) });
    expect(body).not.toHaveProperty('credentials');
  },
);

it.each([401, 403])('reports a refused channel read with its status %s', async (status) => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('{}', { status })));
  const denied = vi.fn();
  render(
    <EventGateway
      session={{
        authenticated: true,
        subject: 'owner',
        permissions: ['cma-eg:admin'],
        expires_at: 9999999999,
      }}
      denied={denied}
    />,
  );
  await waitFor(() => expect(denied).toHaveBeenCalledWith(status));
});

it('puts channels in the sidebar and shows only the selected channel details', async () => {
  const fetcher = vi.fn<typeof fetch>().mockImplementation(async (url) =>
    url === '/api/event-gateway/v1/channels'
      ? new Response(
          JSON.stringify({
            items: [
              {
                record_id: 'legacy-record',
                version: 4,
                provider: { slug: 'cma', level: 'L3' },
                definition: { name: 'Existing channel', sources: [{}] },
              },
              {
                record_id: 'new-record',
                channel_id: 'new-id',
                version: 1,
                provider: { slug: 'cma', level: 'L3' },
                definition: { name: 'Modern channel', sources: [] },
                endpoints: {
                  custom_event_url: 'https://example.test/event',
                  nyxid_callback_url: 'https://example.test/callback',
                },
              },
            ],
          }),
        )
      : new Response('{}', { status: 404 }),
  );
  vi.stubGlobal('fetch', fetcher);
  render(
    <EventGateway
      session={{
        authenticated: true,
        subject: 'owner',
        permissions: ['cma-eg:admin'],
        expires_at: 9999999999,
      }}
      denied={vi.fn()}
    />,
  );
  const sidebar = screen.getByRole('region', { name: 'Channels' });
  expect(screen.getByRole('region', { name: 'Channel workspace' })).toBeVisible();
  expect(screen.queryByRole('heading', { name: 'Existing channel' })).not.toBeInTheDocument();
  await userEvent.click(await within(sidebar).findByRole('button', { name: /Existing channel/ }));
  await screen.findByRole('heading', { name: 'Existing channel' });
  expect(
    await screen.findByRole('link', { name: 'Open existing channel console' }),
  ).toHaveAttribute('href', '/event-gateway/legacy-console/channels/legacy-record');
  expect(screen.queryByRole('button', { name: 'Send test event' })).not.toBeInTheDocument();
  expect(within(sidebar).getByRole('button', { name: /Existing channel/ })).toHaveAttribute(
    'aria-current',
    'page',
  );
  await userEvent.click(within(sidebar).getByRole('button', { name: /Modern channel/ }));
  expect(screen.queryByRole('heading', { name: 'Existing channel' })).not.toBeInTheDocument();
  expect(screen.getByRole('heading', { name: 'Modern channel' })).toBeVisible();
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await userEvent.click(await screen.findByRole('button', { name: /Channel: Modern channel/ }));
  expect(screen.getByRole('button', { name: 'Send test event' })).toBeEnabled();
  expect(screen.getByRole('button', { name: 'Delete channel' })).toBeEnabled();
  expect(within(sidebar).getByRole('button', { name: /Modern channel/ })).toHaveAttribute(
    'aria-current',
    'page',
  );
  expect(fetcher).toHaveBeenCalledTimes(3);
});

it.each(['completed', 'timeout'] as const)(
  'keeps one cold channel test pending through startup and handles %s without retrying',
  async (outcome) => {
    let resolveTest: (response: Response) => void = () => {};
    const fetcher = vi.fn<typeof globalThis.fetch>().mockImplementation(async (url, init) => {
      if (url === '/api/event-gateway/v1/channels/cold-channel/test') {
        return new Promise<Response>((resolve, reject) => {
          resolveTest = resolve;
          init?.signal?.addEventListener('abort', () => reject(init.signal?.reason), {
            once: true,
          });
        });
      }
      return new Response(
        JSON.stringify({
          items: [
            {
              record_id: 'cold-record',
              channel_id: 'cold-channel',
              version: 1,
              provider: { slug: 'cma', level: 'L3' },
              definition: { name: 'Cold channel', sources: [] },
              endpoints: { custom_event_url: '', nyxid_callback_url: '' },
            },
          ],
        }),
      );
    });
    vi.stubGlobal('fetch', fetcher);
    render(
      <EventGateway
        session={{
          authenticated: true,
          subject: 'owner',
          permissions: ['cma-eg:channels:test'],
          expires_at: 9999999999,
        }}
        denied={vi.fn()}
      />,
    );
    await userEvent.click(await screen.findByRole('button', { name: /^Cold channel/ }));
    await userEvent.click(await screen.findByRole('button', { name: /Channel: Cold channel/ }));
    const send = await screen.findByRole('button', { name: 'Send test event' });
    vi.useFakeTimers();
    const timeout = vi.spyOn(AbortSignal, 'timeout').mockImplementation((ms) => {
      const controller = new AbortController();
      setTimeout(
        () => controller.abort(new DOMException('Test request timed out', 'TimeoutError')),
        ms,
      );
      return controller.signal;
    });
    try {
      fireEvent.click(send);
      expect(timeout).toHaveBeenCalledWith(125_000);
      expect(screen.getByRole('button', { name: 'Sending test…' })).toBeDisabled();
      await act(() => vi.advanceTimersByTimeAsync(120_000));
      expect(screen.getByRole('button', { name: 'Sending test…' })).toBeDisabled();
      fireEvent.click(send);
      expect(fetcher).toHaveBeenCalledTimes(3);
      if (outcome === 'completed') {
        await act(async () =>
          resolveTest(new Response(JSON.stringify({ first_text: 'Cold test completed' }))),
        );
        expect(screen.getByText('Cold test completed')).toBeVisible();
      } else {
        await act(() => vi.advanceTimersByTimeAsync(5_000));
        expect(
          screen.getByText(
            'Test result is unknown. Check channel activity before sending another test.',
          ),
        ).toBeVisible();
      }
      expect(screen.getByRole('button', { name: 'Send test event' })).toBeEnabled();
      expect(fetcher).toHaveBeenCalledTimes(3);
      expect(fetcher.mock.calls.filter(([, init]) => init?.method === 'POST')).toHaveLength(1);
    } finally {
      timeout.mockRestore();
      vi.useRealTimers();
    }
  },
);
