import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { EventGateway } from './EventGateway';
import type { Channel, ChannelListItem } from './ChannelEditor';

afterEach(() => vi.unstubAllGlobals());
const session = {
  authenticated: true as const,
  subject: 'owner',
  permissions: ['cma-eg:admin'],
  expires_at: 9999999999,
};
const json = (value: unknown) => new Response(JSON.stringify(value));
function channel(id: string, version = 1): Channel {
  return {
    record_id: `${id}-record`,
    channel_id: `${id}-channel`,
    version,
    provider: { slug: 'cma_codex', level: 'l2' },
    definition: { name: id, sources: [], schema_version: 1 },
    endpoints: {
      custom_event_url: `https://example.test/${id}/events`,
      nyxid_callback_url: `https://example.test/${id}/callback`,
    },
  };
}
// Channels opens on the table; a channel's details appear once it is chosen.
const select = async (name: string) =>
  userEvent.click(
    await within(screen.getByRole('region', { name: 'Channels' })).findByRole('button', {
      name: new RegExp(name),
    }),
  );

it('retains an uncertain edit while switching channels and retries the original version and request', async () => {
  const alpha = channel('Alpha', 7),
    beta = channel('Beta');
  const updated = {
    ...alpha,
    version: 8,
    definition: { ...alpha.definition, name: 'Renamed Alpha' },
  };
  let saves = 0;
  const fetcher = vi.fn<typeof fetch>().mockImplementation(async (url, init) => {
    if (init?.method === 'PUT') {
      expect(url).toBe('/api/event-gateway/v1/channels/Alpha-channel');
      if (++saves === 1) throw new TypeError('Connection interrupted');
      return json(updated);
    }
    return json({ items: [saves > 0 ? updated : alpha, beta] });
  });
  vi.stubGlobal('fetch', fetcher);
  render(<EventGateway session={session} denied={vi.fn()} />);
  await select('Alpha');
  await userEvent.click(await screen.findByRole('button', { name: 'Edit channel' }));
  fireEvent.change(
    within(screen.getByRole('region', { name: 'Alpha details' })).getByLabelText(
      'Channel configuration JSON',
    ),
    { target: { value: JSON.stringify({ name: 'Renamed Alpha', sources: [] }) } },
  );
  await userEvent.click(screen.getByRole('button', { name: 'Save channel' }));
  await screen.findByRole('button', { name: 'Retry same request' });
  // The first request may have committed before its response was lost.
  // Refreshing its new version must preserve the original idempotent retry.
  await userEvent.click(screen.getByRole('button', { name: 'Refresh channels' }));
  await screen.findByRole('heading', { name: 'Renamed Alpha' });
  await select('Beta');
  expect(screen.queryByRole('button', { name: 'Retry same request' })).not.toBeInTheDocument();
  await select('Alpha');
  await userEvent.click(screen.getByRole('button', { name: 'Retry same request' }));
  await screen.findByRole('heading', { name: 'Renamed Alpha' });
  const requests = fetcher.mock.calls.filter(([, init]) => init?.method === 'PUT');
  expect(requests).toHaveLength(2);
  expect(requests[1]?.[1]?.body).toBe(requests[0]?.[1]?.body);
  expect(requests[1]?.[1]?.headers).toEqual(requests[0]?.[1]?.headers);
  expect(JSON.parse(String(requests[0]?.[1]?.body))).toMatchObject({
    expected_version: 7,
    name: 'Renamed Alpha',
  });
  expect(screen.queryByRole('button', { name: 'Save channel' })).not.toBeInTheDocument();
});

it('retains an uncertain create across navigation and removes a freshly created channel after deletion', async () => {
  const existing = channel('Existing'),
    created = channel('Created', 2);
  let creates = 0;
  const fetcher = vi.fn<typeof fetch>().mockImplementation(async (url, init) => {
    if (init?.method === 'POST') {
      expect(url).toBe('/api/event-gateway/v1/channels');
      if (++creates === 1) throw new TypeError('Connection interrupted');
      return json(created);
    }
    if (init?.method === 'DELETE') return new Response(null, { status: 204 });
    // A lagging list response must not hide the successful create or resurrect its deletion.
    return json({ items: [existing] });
  });
  vi.stubGlobal('fetch', fetcher);
  render(<EventGateway session={session} denied={vi.fn()} />);
  await select('Existing');
  await screen.findByRole('heading', { name: 'Existing' });
  await userEvent.click(screen.getByRole('button', { name: 'New channel' }));
  const form = screen.getByRole('button', { name: 'Create channel' }).closest('form')!;
  fireEvent.change(within(form).getByLabelText('Channel configuration JSON'), {
    target: { value: JSON.stringify({ name: 'Created', sources: [] }) },
  });
  fireEvent.change(within(form).getByLabelText('Agent key'), { target: { value: 'fixture-key' } });
  await userEvent.click(screen.getByRole('button', { name: 'Create channel' }));
  await screen.findByRole('button', { name: 'Retry same request' });
  await select('Existing');
  await userEvent.click(screen.getByRole('button', { name: 'New channel' }));
  expect(within(form).getByLabelText('Channel configuration JSON')).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Retry same request' }));
  await screen.findByRole('heading', { name: 'Created' });
  const requests = fetcher.mock.calls.filter(([, init]) => init?.method === 'POST');
  expect(requests).toHaveLength(2);
  expect(requests[1]?.[1]?.body).toBe(requests[0]?.[1]?.body);
  expect(requests[1]?.[1]?.headers).toEqual(requests[0]?.[1]?.headers);
  await userEvent.click(screen.getByRole('button', { name: 'Delete channel' }));
  await userEvent.click(screen.getByRole('button', { name: 'Confirm delete' }));
  await waitFor(() =>
    expect(screen.queryByRole('heading', { name: 'Created' })).not.toBeInTheDocument(),
  );
  // The deleted channel was open, so the page returns to the table of what remains.
  const table = screen.getByRole('table', { name: 'Your channels' });
  expect(within(table).getByRole('button', { name: 'Existing' })).toBeVisible();
  expect(within(table).queryByRole('button', { name: 'Created' })).not.toBeInTheDocument();
  expect(
    within(screen.getByRole('region', { name: 'Channels' })).queryByRole('button', {
      name: /Created/,
    }),
  ).not.toBeInTheDocument();
  expect(fetcher.mock.calls.filter(([, init]) => init?.method === 'DELETE')).toEqual([
    [
      '/api/event-gateway/v1/channels/Created-channel?expected_version=2',
      expect.objectContaining({ method: 'DELETE' }),
    ],
  ]);
});

it('reopens a legacy record only with its original channel ID and rejects an unrelated record', async () => {
  const original = channel('Legacy', 4);
  const legacy: ChannelListItem = {
    record_id: original.record_id,
    version: original.version,
    provider: original.provider,
    definition: original.definition,
  };
  const fetcher = vi.fn<typeof fetch>().mockImplementation(async (url) => {
    if (url === '/api/event-gateway/v1/channels') return json({ items: [legacy] });
    if (url === '/api/event-gateway/v1/channels/Legacy-channel') return json(original);
    if (url === '/api/event-gateway/v1/channels/Wrong-channel') return json(channel('Wrong'));
    return new Response('{}', { status: 404 });
  });
  vi.stubGlobal('fetch', fetcher);
  render(<EventGateway session={session} denied={vi.fn()} />);
  await select('Legacy');
  const input = await screen.findByLabelText('Original channel ID');
  fireEvent.change(input, { target: { value: 'Wrong-channel' } });
  await userEvent.click(screen.getByRole('button', { name: 'Open existing channel' }));
  expect(await screen.findByRole('alert')).toHaveTextContent('belongs to another channel');
  expect(screen.queryByRole('button', { name: 'Send test event' })).not.toBeInTheDocument();
  fireEvent.change(screen.getByLabelText('Original channel ID'), {
    target: { value: 'Legacy-channel' },
  });
  await userEvent.click(screen.getByRole('button', { name: 'Open existing channel' }));
  expect(await screen.findByRole('button', { name: 'Send test event' })).toBeEnabled();
  expect(screen.getByText(original.endpoints.custom_event_url)).toBeVisible();
  expect(fetcher.mock.calls.every(([, init]) => init?.method === undefined)).toBe(true);
  expect(fetcher.mock.calls.map(([url]) => url)).toEqual([
    '/api/event-gateway/v1/channels',
    '/api/event-gateway/v1/channel-records/Legacy-record',
    '/api/event-gateway/v1/channels/Wrong-channel',
    '/api/event-gateway/v1/channels/Legacy-channel',
  ]);
});

it('recovers legacy endpoints automatically when the original creation record is available', async () => {
  const original = channel('Recovered');
  const legacy: ChannelListItem = {
    record_id: original.record_id,
    version: original.version,
    provider: original.provider,
    definition: original.definition,
  };
  const fetcher = vi
    .fn<typeof fetch>()
    .mockImplementation(async (url) =>
      json(url === '/api/event-gateway/v1/channels' ? { items: [legacy] } : original),
    );
  vi.stubGlobal('fetch', fetcher);
  render(<EventGateway session={session} denied={vi.fn()} />);
  await select('Recovered');
  expect(await screen.findByRole('button', { name: 'Edit channel' })).toBeEnabled();
  expect(screen.queryByLabelText('Original channel ID')).not.toBeInTheDocument();
  expect(fetcher).toHaveBeenCalledTimes(2);
});
