import { act, fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { EventGateway } from './EventGateway';
// Load the real graph module before exercising lifecycle and editor interactions.
import './DeliveryFlowCanvas';
import type { Channel, ChannelListItem } from './ChannelEditor';

afterEach(() => vi.unstubAllGlobals());
const session = {
  authenticated: true as const,
  subject: 'owner',
  permissions: ['cma-eg:admin'],
  expires_at: 9999999999,
};
const json = (value: unknown) => new Response(JSON.stringify(value));

it.each(['keyboard', 'pointer'])(
  'returns from a channel draft to the landing page by %s',
  async (input) => {
    const actor = userEvent.setup();
    vi.stubGlobal(
      'fetch',
      vi.fn().mockImplementation(async () => json({ items: [channel('Existing')] })),
    );
    render(<EventGateway session={session} denied={vi.fn()} />);
    const sidebar = within(screen.getByRole('region', { name: 'Channels' }));
    await actor.click(await sidebar.findByRole('button', { name: /^Existing/ }));
    await actor.click(screen.getByRole('button', { name: 'New channel' }));
    const draft = screen.getByRole('textbox', { name: 'Channel name' });
    await actor.type(draft, 'Retained draft');
    const crumb = within(screen.getByRole('navigation', { name: 'Breadcrumb' })).getByRole('link', {
      name: 'Event Gateway',
    });
    expect(crumb).toHaveAttribute('href', '/event-gateway');
    if (input === 'keyboard') {
      crumb.focus();
      expect(crumb).toHaveFocus();
      await actor.keyboard('{Enter}');
    } else {
      await actor.click(crumb);
    }
    expect(screen.getByRole('region', { name: 'Channel workspace' })).toBeVisible();
    expect(screen.queryByRole('heading', { name: 'Existing' })).not.toBeInTheDocument();
    await actor.click(
      within(screen.getByRole('region', { name: 'Channel workspace' })).getByRole('button', {
        name: 'New channel',
      }),
    );
    expect(screen.getByRole('textbox', { name: 'Channel name' })).toHaveValue('Retained draft');
  },
);

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
const select = async (name: string) => {
  const dialog = screen.queryByRole('dialog');
  if (dialog) await userEvent.click(within(dialog).getByRole('button', { name: 'Close' }));
  await userEvent.click(
    await within(screen.getByRole('region', { name: 'Channels' })).findByRole('button', {
      name: new RegExp(name),
    }),
  );
  if (screen.queryByRole('region', { name: 'Delivery map' }))
    await userEvent.click(
      await screen.findByRole('button', { name: new RegExp(`Channel: .*${name}`) }),
    );
};

// The fixture owns the responses. Settle them before querying the committed UI.
async function clickAndSettle(button: HTMLElement) {
  expect(button).toBeEnabled();
  await act(async () => {
    fireEvent.click(button);
  });
}

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
  vi.stubGlobal('EventSource', undefined);
  await act(async () => {
    render(<EventGateway session={session} denied={vi.fn()} />);
  });
  const channels = () => within(screen.getByRole('region', { name: 'Channels' }));
  const details = (name: string) => within(screen.getByRole('region', { name: `${name} details` }));
  async function openSettings(name: string) {
    fireEvent.click(channels().getByRole('button', { name: new RegExp(`^${name}`) }));
    const map = within(details(name).getByRole('region', { name: 'Delivery map' }));
    fireEvent.click(await map.findByRole('button', { name: new RegExp(`^Channel: ${name}`) }));
    return within(screen.getByRole('dialog', { name: 'Channel settings' }));
  }
  const alphaSettings = await openSettings('Alpha');
  fireEvent.change(alphaSettings.getByLabelText('Channel name'), {
    target: { value: 'Renamed Alpha' },
  });
  await clickAndSettle(alphaSettings.getByRole('button', { name: 'Save channel' }));
  expect(alphaSettings.getByRole('button', { name: 'Retry same request' })).toBeEnabled();
  // The first request may have committed before its response was lost.
  // Refreshing its new version must preserve the original idempotent retry.
  fireEvent.click(alphaSettings.getByRole('button', { name: 'Close' }));
  await clickAndSettle(channels().getByRole('button', { name: 'Refresh channels' }));
  expect(details('Renamed Alpha').getByRole('heading', { name: 'Renamed Alpha' })).toBeVisible();
  const betaSettings = await openSettings('Beta');
  expect(screen.queryByRole('button', { name: 'Retry same request' })).not.toBeInTheDocument();
  fireEvent.click(betaSettings.getByRole('button', { name: 'Close' }));
  const resumedSettings = await openSettings('Renamed Alpha');
  await clickAndSettle(resumedSettings.getByRole('button', { name: 'Retry same request' }));
  expect(details('Renamed Alpha').getByRole('heading', { name: 'Renamed Alpha' })).toBeVisible();
  const requests = fetcher.mock.calls.filter(([, init]) => init?.method === 'PUT');
  expect(requests).toHaveLength(2);
  expect(requests[1]?.[1]?.body).toBe(requests[0]?.[1]?.body);
  expect(requests[1]?.[1]?.headers).toEqual(requests[0]?.[1]?.headers);
  expect(JSON.parse(String(requests[0]?.[1]?.body))).toMatchObject({
    expected_version: 7,
    name: 'Renamed Alpha',
  });
  expect(resumedSettings.getByRole('button', { name: 'Save channel' })).toBeEnabled();
  expect(resumedSettings.getByText('Changes saved.')).toBeVisible();
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
  vi.stubGlobal('EventSource', undefined);
  await act(async () => {
    render(<EventGateway session={session} denied={vi.fn()} />);
  });
  const channels = within(screen.getByRole('region', { name: 'Channels' }));
  const existingChannel = channels.getByRole('button', { name: /^Existing/ });
  const openExisting = () => fireEvent.click(existingChannel);
  openExisting();
  expect(screen.getByRole('heading', { name: 'Existing' })).toBeVisible();
  const newChannel = screen.getByRole('button', { name: 'New channel' });
  fireEvent.click(newChannel);
  const form = screen.getByRole('button', { name: 'Create channel' }).closest('form')!;
  const editor = within(form.parentElement!);
  fireEvent.change(within(form).getByLabelText('Channel name'), {
    target: { value: 'Created' },
  });
  fireEvent.change(within(form).getByLabelText('Provider agent key'), {
    target: { value: 'fixture-key' },
  });
  fireEvent.change(within(form).getByLabelText('NyxID issuer'), {
    target: { value: 'https://nyx.example' },
  });
  fireEvent.change(within(form).getByLabelText('Signing key ID'), {
    target: { value: 'fixture-kid' },
  });
  fireEvent.change(within(form).getByLabelText('Route IDs 1'), {
    target: { value: 'fixture-route' },
  });
  await clickAndSettle(within(form).getByRole('button', { name: 'Create channel' }));
  expect(editor.getByRole('button', { name: 'Retry same request' })).toBeVisible();
  openExisting();
  expect(form).not.toBeVisible();
  expect(screen.getByRole('heading', { name: 'Existing' })).toBeVisible();
  fireEvent.click(newChannel);
  expect(form).toBeVisible();
  expect(within(form).getByLabelText('Channel name')).toBeDisabled();
  await clickAndSettle(editor.getByRole('button', { name: 'Retry same request' }));
  const details = within(screen.getByRole('region', { name: 'Created details' }));
  expect(details.getByRole('heading', { name: 'Created' })).toBeVisible();
  const requests = fetcher.mock.calls.filter(([, init]) => init?.method === 'POST');
  expect(requests).toHaveLength(2);
  expect(requests[1]?.[1]?.body).toBe(requests[0]?.[1]?.body);
  expect(requests[1]?.[1]?.headers).toEqual(requests[0]?.[1]?.headers);
  // The delivery graph is lazy-loaded. Wait for its control in the new map.
  const map = within(details.getByRole('region', { name: 'Delivery map' }));
  fireEvent.click(await map.findByRole('button', { name: /Channel: Created/ }));
  const settings = within(screen.getByRole('dialog', { name: 'Channel settings' }));
  fireEvent.click(settings.getByRole('button', { name: 'Delete channel' }));
  await clickAndSettle(settings.getByRole('button', { name: 'Confirm delete' }));
  expect(screen.queryByRole('heading', { name: 'Created' })).not.toBeInTheDocument();
  expect(screen.getByRole('region', { name: 'Channel workspace' })).toBeVisible();
  expect(channels.queryByRole('button', { name: /^Created/ })).not.toBeInTheDocument();
  expect(fetcher.mock.calls.filter(([, init]) => init?.method === 'DELETE')).toEqual([
    [
      '/api/event-gateway/v1/channels/Created-channel?expected_version=2',
      expect.objectContaining({ method: 'DELETE' }),
    ],
  ]);
}, 15_000);

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
  await userEvent.click(await screen.findByRole('button', { name: /Channel: Legacy/ }));
  expect(await screen.findByRole('button', { name: 'Send test event' })).toBeEnabled();
  expect(screen.getByText(original.endpoints.custom_event_url)).toBeVisible();
  expect(fetcher.mock.calls.every(([, init]) => init?.method === undefined)).toBe(true);
  expect(fetcher.mock.calls.map(([url]) => url)).toEqual([
    '/api/event-gateway/v1/channels',
    '/api/event-gateway/v1/channel-capabilities',
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
  await userEvent.click(await screen.findByRole('button', { name: /Channel: Recovered/ }));
  expect(await screen.findByLabelText('Channel name')).toBeEnabled();
  expect(screen.queryByLabelText('Original channel ID')).not.toBeInTheDocument();
  expect(fetcher).toHaveBeenCalledTimes(3);
});

it('moves keyboard focus to the create heading and back to the landing action on cancellation', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn().mockImplementation(async () => json({ items: [] })),
  );
  const user = userEvent.setup();
  render(<EventGateway session={session} denied={vi.fn()} />);
  const action = () =>
    within(screen.getByRole('region', { name: 'Channel workspace' })).getByRole('button', {
      name: 'New channel',
    });
  for (let count = 0; count < 2; count++) {
    action().focus();
    await user.keyboard('{Enter}');
    expect(screen.getByRole('heading', { name: 'New channel' })).toHaveFocus();
    const cancel = screen.getByRole('button', { name: 'Cancel' });
    cancel.focus();
    await user.keyboard('{Enter}');
    expect(action()).toHaveFocus();
  }
});
