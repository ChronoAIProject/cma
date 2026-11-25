import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import type { ComponentProps } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelNodeTests } from './ChannelNodeTests';
import { ChannelOutboundMessage } from './ChannelOutboundMessage';
import { RelayCapabilitiesContext, type RelayDiscovery } from './RelayCapabilities';
import { outboundStorageKey } from './outboundJournal';
import type { Channel } from './channelTypes';

const captured = vi.hoisted(() => new Map<string, () => void>());
vi.mock('../ui/Button', async (importOriginal) => {
  const { Button } = await importOriginal<typeof import('../ui/Button')>();
  return {
    Button: (props: ComponentProps<typeof Button>) => {
      if (typeof props.children === 'string' && props.onClick && !captured.has(props.children))
        captured.set(props.children, () => props.onClick!({} as never));
      return <Button {...props} />;
    },
  };
});
const channel: Channel = {
  record_id: 'lifecycle-record',
  channel_id: 'original',
  version: 1,
  provider: { slug: 'cma', level: 'l3' },
  definition: {
    name: 'Channel',
    sources: [{ type: 'nyxid_relay', platform: 'telegram', route_ids: ['saved-route'] }],
  },
  endpoints: { custom_event_url: '', nyxid_callback_url: '' },
};
const capabilities = {
  status: 'ready',
  authoring: { outbound_messages_writes_enabled: true },
} as RelayDiscovery;
const receipt = {
  delivery_id: 'delivery',
  route_id: 'saved-route',
  status: 'prepared',
  delivery: null,
};
const json = (value: unknown) => new Response(JSON.stringify(value));
afterEach(() => {
  captured.clear();
  vi.unstubAllGlobals();
});

it('rejects an already captured test callback when channel readiness changes or its owner unmounts', async () => {
  const fetcher = vi.fn<typeof fetch>(async () => json({ first_text: 'ok' }));
  vi.stubGlobal('fetch', fetcher);
  const props = { channelId: 'original', selection: { kind: 'channel' as const }, denied: vi.fn() };
  const view = render(<ChannelNodeTests {...props} writable />);
  const staleTest = captured.get('Send test event')!;
  view.rerender(<ChannelNodeTests {...props} writable={false} />);
  await act(async () => {
    staleTest();
  });
  expect(fetcher).not.toHaveBeenCalled();
  view.rerender(<ChannelNodeTests {...props} writable />);
  fireEvent.click(screen.getByRole('button', { name: 'Send test event' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  view.unmount();
  await act(async () => {
    staleTest();
  });
  expect(fetcher).toHaveBeenCalledOnce();
});

function composer(writable: boolean) {
  return (
    <RelayCapabilitiesContext value={capabilities}>
      <ChannelOutboundMessage
        owner="owner"
        channel={channel}
        selection={{ kind: 'producer', index: 0 }}
        denied={vi.fn()}
        writable={writable}
      />
    </RelayCapabilitiesContext>
  );
}
it('keeps outbound draft and preparation identity while blocking stale prepare/send callbacks', async () => {
  const fetcher = vi.fn<typeof fetch>(async () => json(receipt));
  vi.stubGlobal('fetch', fetcher);
  const view = render(composer(true));
  fireEvent.change(screen.getByLabelText('Message text'), {
    target: { value: 'Retained message' },
  });
  const stalePrepare = captured.get('Review message')!;
  view.rerender(composer(false));
  await act(async () => {
    stalePrepare();
  });
  expect(screen.getByLabelText('Message text')).toHaveValue('Retained message');
  expect(fetcher).not.toHaveBeenCalled();
  view.rerender(composer(true));
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  await screen.findByRole('button', { name: 'Send message' });
  const saved = sessionStorage.getItem(outboundStorageKey('owner', channel.record_id));
  const staleSend = captured.get('Send message')!;
  view.rerender(composer(false));
  await act(async () => {
    staleSend();
  });
  expect(fetcher).toHaveBeenCalledOnce();
  expect(sessionStorage.getItem(outboundStorageKey('owner', channel.record_id))).toBe(saved);
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  view.rerender(composer(true));
  expect(fetcher).toHaveBeenCalledOnce();
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));
  expect(fetcher.mock.calls[1]?.[0]).toBe(
    '/api/event-gateway/v1/channels/original/outbound-messages/delivery/send',
  );
  expect(JSON.parse(String(fetcher.mock.calls[1]?.[1]?.body))).toMatchObject({
    text: 'Retained message',
  });
});

it('retains an uncertain outbound preparation and retries its exact key/body only when allowed', async () => {
  let failed = true;
  const fetcher = vi.fn<typeof fetch>(async () => {
    if (failed) throw new TypeError('Unknown');
    return json(receipt);
  });
  vi.stubGlobal('fetch', fetcher);
  const view = render(composer(true));
  fireEvent.change(screen.getByLabelText('Message text'), {
    target: { value: 'Uncertain message' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Check saved request' })).toBeEnabled(),
  );
  const first = fetcher.mock.calls[0]!;
  view.rerender(composer(false));
  await act(async () => {
    captured.get('Check saved request')!();
  });
  expect(fetcher).toHaveBeenCalledOnce();
  expect(screen.getByRole('button', { name: 'Check saved request' })).toBeDisabled();
  failed = false;
  view.rerender(composer(true));
  fireEvent.click(screen.getByRole('button', { name: 'Check saved request' }));
  await screen.findByRole('button', { name: 'Send message' });
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(fetcher.mock.calls[1]?.[0]).toBe(first[0]);
  expect(fetcher.mock.calls[1]?.[1]).toMatchObject({
    method: first[1]?.method,
    body: first[1]?.body,
    headers: first[1]?.headers,
  });
});
