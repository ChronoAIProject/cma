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
const receipt = (status = 'prepared', delivery: unknown = null) => ({
  delivery_id: 'delivery',
  route_id: 'saved-route',
  status,
  delivery,
});
const json = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status });
const discovery = (media?: boolean, outbound = true) =>
  ({
    status: 'ready',
    authoring: {
      outbound_messages_writes_enabled: outbound,
      ...(media === undefined ? {} : { native_media_writes_enabled: media }),
    },
  }) as RelayDiscovery;
function content(media?: boolean, outbound = true) {
  return (
    <RelayCapabilitiesContext value={discovery(media, outbound)}>
      <ChannelOutboundMessage
        owner="alice"
        channel={channel}
        selection={{ kind: 'producer', index: 0 }}
        denied={vi.fn()}
      />
    </RelayCapabilitiesContext>
  );
}
function add(url = 'https://media.example/image.png') {
  fireEvent.click(screen.getByRole('button', { name: 'Add media attachment' }));
  const urls = screen.getAllByLabelText(/^HTTPS URL /);
  fireEvent.change(urls.at(-1)!, { target: { value: url } });
}
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

it('reviews and explicitly sends four typed media attachments with exact optional fields and no text', async () => {
  const timeout = vi.spyOn(AbortSignal, 'timeout');
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(json(receipt(), 201))
    .mockResolvedValueOnce(json(receipt('committed', { outcome: 'confirmed' })));
  vi.stubGlobal('fetch', fetch);
  render(content(true));
  for (const [index, kind] of ['Image', 'File', 'Audio', 'Video'].entries()) {
    add(`https://media.example/${kind.toLowerCase()}`);
    fireEvent.click(screen.getByRole('button', { name: `Media type ${index + 1}` }));
    fireEvent.click(screen.getByRole('menuitemradio', { name: kind }));
  }
  fireEvent.change(screen.getByLabelText('Filename 2 (optional)'), {
    target: { value: 'report.pdf' },
  });
  fireEvent.change(screen.getByLabelText('MIME type 2 (optional)'), {
    target: { value: 'application/pdf' },
  });
  fireEvent.change(screen.getByLabelText('Caption 2 (optional)'), {
    target: { value: 'Quarterly report' },
  });
  expect(screen.getByRole('button', { name: 'Add media attachment' })).toBeDisabled();
  expect(fetch).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  await screen.findByRole('button', { name: 'Send message' });
  const request = JSON.parse(fetch.mock.calls[0]![1]!.body as string);
  expect(request.message).toEqual({
    native_attachments: [
      { kind: 'image', source: { type: 'url', url: 'https://media.example/image' } },
      {
        kind: 'file',
        source: { type: 'url', url: 'https://media.example/file' },
        filename: 'report.pdf',
        mime_type: 'application/pdf',
        caption: 'Quarterly report',
      },
      { kind: 'audio', source: { type: 'url', url: 'https://media.example/audio' } },
      { kind: 'video', source: { type: 'url', url: 'https://media.example/video' } },
    ],
  });
  expect(screen.getByText('Caption: Quarterly report')).toBeVisible();
  expect(fetch).toHaveBeenCalledTimes(1);
  const saved = JSON.parse(sessionStorage.getItem(outboundStorageKey('alice', 'record'))!);
  expect(saved.version).toBe(2);
  expect(saved.intent.draft.body).toEqual(request);
  fireEvent.click(screen.getByRole('button', { name: 'Send message' }));
  await screen.findByText('The platform accepted the message.');
  expect(fetch.mock.calls[1]![1]!.body).toBe(JSON.stringify(request.message));
  expect(timeout).toHaveBeenNthCalledWith(1, 15000);
  expect(timeout).toHaveBeenNthCalledWith(2, 65000);
});

it.each([
  'http://media.example/image.png',
  'https://user:password@media.example/file',
  'https://media.example/file#fragment',
  'https://media.example/file#',
  'https://media.example/some image.png',
  'file:///home/photo.png',
  'not a URL',
])('blocks invalid media URL %s before saving or making requests', (url) => {
  const fetch = vi.fn();
  vi.stubGlobal('fetch', fetch);
  render(content(true));
  add(url);
  expect(screen.getByText(/Enter an HTTPS URL/)).toBeVisible();
  expect(screen.getByRole('button', { name: 'Review message' })).toBeDisabled();
  expect(sessionStorage.getItem(outboundStorageKey('alice', 'record'))).toBeNull();
  expect(fetch).not.toHaveBeenCalled();
});

it('removes an attachment without changing the other typed records', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(json(receipt(), 201)));
  render(content(true));
  add('https://media.example/first');
  add('https://media.example/second');
  fireEvent.change(screen.getByLabelText('Caption 2 (optional)'), { target: { value: 'Keep me' } });
  fireEvent.click(screen.getByRole('button', { name: 'Remove attachment 1' }));
  expect(screen.getByLabelText('HTTPS URL 1')).toHaveValue('https://media.example/second');
  expect(screen.getByLabelText('Caption 1 (optional)')).toHaveValue('Keep me');
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  await screen.findByRole('button', { name: 'Send message' });
  expect(
    JSON.parse(vi.mocked(fetch).mock.calls[0]![1]!.body as string).message.native_attachments,
  ).toHaveLength(1);
});

it('recovers an uncertain media review after reload with the exact request and idempotency key', async () => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockRejectedValueOnce(new TypeError('lost'))
    .mockResolvedValueOnce(json(receipt(), 201));
  vi.stubGlobal('fetch', fetch);
  const mounted = render(content(true));
  add('https://media.example/asset?version=original');
  fireEvent.change(screen.getByLabelText('Message text'), { target: { value: 'Exact text' } });
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  await screen.findByText('lost');
  mounted.unmount();
  render(content(true));
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(screen.getByText('https://media.example/asset?version=original')).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Check saved request' }));
  await screen.findByRole('button', { name: 'Send message' });
  expect(fetch.mock.calls[1]![1]!.body).toBe(fetch.mock.calls[0]![1]!.body);
  expect(fetch.mock.calls[1]![1]!.headers).toEqual(fetch.mock.calls[0]![1]!.headers);
});

it('keeps saved media review and status access when both write gates disappear', async () => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(json(receipt(), 201))
    .mockResolvedValueOnce(json(receipt()));
  vi.stubGlobal('fetch', fetch);
  const mounted = render(content(true));
  add();
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  await screen.findByRole('button', { name: 'Send message' });
  mounted.unmount();
  render(content(false, false));
  expect(screen.getByText('https://media.example/image.png')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Send message' })).toBeDisabled();
  expect(screen.queryByRole('button', { name: 'Add media attachment' })).not.toBeInTheDocument();
  expect(fetch).toHaveBeenCalledTimes(1);
  fireEvent.click(screen.getByRole('button', { name: 'Check delivery status' }));
  await waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
  expect(fetch.mock.calls[1]![1]!.method).toBe('GET');
  expect(
    JSON.parse(sessionStorage.getItem(outboundStorageKey('alice', 'record'))!).intent.draft.body
      .message.native_attachments,
  ).toHaveLength(1);
});

it('hides unavailable media composition without changing ordinary text review', () => {
  const mounted = render(content());
  expect(screen.queryByRole('button', { name: 'Add media attachment' })).not.toBeInTheDocument();
  fireEvent.change(screen.getByLabelText('Message text'), { target: { value: 'Retained draft' } });
  mounted.rerender(content(true));
  expect(screen.getByLabelText('Message text')).toHaveValue('Retained draft');
  add();
  mounted.rerender(content(false));
  expect(screen.queryByRole('button', { name: 'Add media attachment' })).not.toBeInTheDocument();
  expect(screen.getByText('https://media.example/image.png')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Review message' })).toBeDisabled();
  mounted.rerender(content(true));
  expect(screen.getByLabelText('HTTPS URL 1')).toHaveValue('https://media.example/image.png');
});

it('retains version 1 text journals without converting their request shape', async () => {
  const draft = {
    key: 'legacy-key',
    body: {
      source_id: 'relay_telegram',
      route_id: 'saved-route',
      message: { text: 'Legacy text' },
    },
  };
  sessionStorage.setItem(
    outboundStorageKey('alice', 'record'),
    JSON.stringify({ version: 1, intent: { draft, uncertain: true } }),
  );
  const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValueOnce(json(receipt(), 201));
  vi.stubGlobal('fetch', fetch);
  render(content(true));
  expect(fetch).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Check saved request' }));
  await screen.findByRole('button', { name: 'Send message' });
  expect(fetch.mock.calls[0]![1]!.body).toBe(JSON.stringify(draft.body));
  expect(JSON.parse(sessionStorage.getItem(outboundStorageKey('alice', 'record'))!).version).toBe(
    1,
  );
});

it.each([
  ['Filename 1 (optional)', '../report.pdf', /without a folder path/],
  ['MIME type 1 (optional)', 'image/png; charset=utf-8', /lowercase MIME type/],
  ['Caption 1 (optional)', 'x'.repeat(1025), /Caption must contain/],
])('blocks invalid %s before review', (field, value, notice) => {
  const fetch = vi.fn();
  vi.stubGlobal('fetch', fetch);
  render(content(true));
  add();
  fireEvent.change(screen.getByLabelText(field), { target: { value } });
  expect(screen.getByText(notice)).toBeVisible();
  expect(screen.getByRole('button', { name: 'Review message' })).toBeDisabled();
  expect(fetch).not.toHaveBeenCalled();
});

it('retains media commit uncertainty across reload and never offers an automatic resend', async () => {
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValueOnce(json(receipt(), 201))
    .mockImplementationOnce(async () => {
      const stored = JSON.parse(sessionStorage.getItem(outboundStorageKey('alice', 'record'))!);
      expect(stored.version).toBe(2);
      expect(stored.intent.uncertain).toBe(true);
      expect(stored.intent.draft.body.message.native_attachments[0].source.url).toBe(
        'https://media.example/image.png',
      );
      return await new Promise<Response>(() => {});
    });
  vi.stubGlobal('fetch', fetch);
  const mounted = render(content(true));
  add();
  fireEvent.click(screen.getByRole('button', { name: 'Review message' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Send message' }));
  await waitFor(() => expect(fetch).toHaveBeenCalledTimes(2));
  mounted.unmount();
  render(content(true));
  expect(screen.getByText('https://media.example/image.png')).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Send message' })).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Check delivery status' })).toBeEnabled();
  expect(fetch).toHaveBeenCalledTimes(2);
});

it('refuses malformed retained media without discarding its journal or sending', () => {
  const stored = JSON.stringify({
    version: 2,
    intent: {
      draft: {
        key: 'saved-media',
        body: {
          source_id: 'relay_telegram',
          route_id: 'saved-route',
          message: {
            native_attachments: [
              { kind: 'image', source: { type: 'url', url: 'http://invalid.example/image' } },
            ],
          },
        },
      },
      uncertain: true,
    },
  });
  sessionStorage.setItem(outboundStorageKey('alice', 'record'), stored);
  const fetch = vi.fn();
  vi.stubGlobal('fetch', fetch);
  render(content(true));
  expect(screen.getByText(/saved message could not be read/)).toBeVisible();
  expect(screen.getByRole('button', { name: 'Review message' })).toBeDisabled();
  expect(sessionStorage.getItem(outboundStorageKey('alice', 'record'))).toBe(stored);
  expect(fetch).not.toHaveBeenCalled();
});
