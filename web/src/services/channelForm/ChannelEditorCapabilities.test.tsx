import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor, type Channel } from '../ChannelEditor';
import { RelayCapabilitiesContext, type RelayDiscovery } from '../RelayCapabilities';
afterEach(() => vi.unstubAllGlobals());
const channel: Channel = {
  channel_id: 'owned-channel',
  record_id: 'owned-record',
  version: 7,
  provider: { slug: 'cma_codex', level: 'l3' },
  definition: {
    name: 'Owned',
    sources: [
      {
        type: 'nyxid_relay',
        platform: 'discord',
        issuer: 'https://nyxid.example.test',
        key_id: 'key',
        route_ids: ['route'],
        verify_raw_event: true,
        admission: { type: 'all' },
      },
    ],
  },
  endpoints: { custom_event_url: '', nyxid_callback_url: '' },
};
const available: RelayDiscovery = {
  status: 'ready',
  authoring: {
    relay_raw_verification_writes_enabled: true,
    dynamic_sources_writes_enabled: false,
    multi_consumer_writes_enabled: false,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [
      {
        id: 'discord',
        label: 'Discord',
        verification_modes: [{ id: 'raw_event', writes_enabled: true }],
      },
    ],
  },
};
it('retries the exact uncertain request even if capability discovery changes', async () => {
  const fetcher = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('Connection interrupted'))
    .mockResolvedValueOnce(new Response(JSON.stringify({ ...channel, version: 8 })));
  vi.stubGlobal('fetch', fetcher);
  const view = (discovery: RelayDiscovery) => (
    <RelayCapabilitiesContext value={discovery}>
      <ChannelEditor
        channel={channel}
        section={{ kind: 'producer', index: 0 }}
        onDone={vi.fn()}
        denied={vi.fn()}
      />
    </RelayCapabilitiesContext>
  );
  const { rerender } = render(view(available));
  fireEvent.change(screen.getByLabelText('Bot username'), { target: { value: 'changed' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  await screen.findByRole('button', { name: 'Retry same request' });
  rerender(view({ status: 'legacy' }));
  fireEvent.click(screen.getByRole('button', { name: 'Retry same request' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));
  expect(fetcher.mock.calls[1]?.[0]).toBe(fetcher.mock.calls[0]?.[0]);
  expect(fetcher.mock.calls[1]?.[1].body).toBe(fetcher.mock.calls[0]?.[1].body);
  expect(fetcher.mock.calls[1]?.[1].headers).toEqual(fetcher.mock.calls[0]?.[1].headers);
});
it('retains an unsupported raw draft and does not send or downgrade it', () => {
  const fetcher = vi.fn();
  vi.stubGlobal('fetch', fetcher);
  render(
    <ChannelEditor
      channel={channel}
      section={{ kind: 'producer', index: 0 }}
      onDone={vi.fn()}
      denied={vi.fn()}
    />,
  );
  fireEvent.change(screen.getByLabelText('Bot username'), { target: { value: 'retained draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save producer' }));
  expect(screen.getByRole('alert')).toHaveTextContent('Your draft has been retained');
  expect(screen.getByLabelText('Bot username')).toHaveValue('retained draft');
  expect(screen.getByRole('button', { name: 'Incoming message verification' })).toHaveTextContent(
    'Verify original platform event',
  );
  expect(fetcher).not.toHaveBeenCalled();
});
