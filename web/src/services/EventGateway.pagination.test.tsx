import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { EventGateway } from './EventGateway';
// Load the real graph module before exercising lifecycle and editor interactions.
import './DeliveryFlowCanvas';
import { channelIsWritable, channelResponseMatches, mergeChannelEvidence } from './channelEvidence';
import type { Channel } from './channelTypes';

const session = {
  authenticated: true as const,
  subject: 'owner',
  permissions: ['cma-eg:admin'],
  expires_at: 9999999999,
};
const record = (id: string, name: string): Channel => ({
  record_id: id,
  channel_id: `${id}-address`,
  version: 7,
  provider: { slug: 'cma_codex', level: 'l2' },
  definition: {
    name,
    sources: [],
    schema_version: 1,
    creation: { ready: true },
    owner: { kind: 'human', subject: 'owner' },
  },
  endpoints: {
    custom_event_url: 'https://example.test/events',
    nyxid_callback_url: 'https://example.test/callback',
  },
});
const json = (value: unknown) => new Response(JSON.stringify(value));
afterEach(() => vi.unstubAllGlobals());

it('keeps a second-page editor and exact uncertain PUT through failed and successful status refreshes', async () => {
  const first = record('first', 'First page'),
    second = record('second', 'Second page');
  const writes: { path: string; request: RequestInit }[] = [];
  let status = 200;
  vi.stubGlobal('EventSource', undefined);
  vi.stubGlobal(
    'fetch',
    vi.fn<typeof fetch>(async (url, init) => {
      if (init?.method === 'PUT') {
        writes.push({ path: String(url), request: init });
        throw new TypeError('Uncertain save');
      }
      if (String(url).includes('/v1/channels?after='))
        return json({ items: [second], next_cursor: null });
      if (url === '/api/event-gateway/v1/channels')
        return new Response(JSON.stringify({ items: [first], next_cursor: 'first' }), { status });
      return json({});
    }),
  );
  render(<EventGateway session={session} denied={vi.fn()} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Load more channels' }));
  fireEvent.click(
    await within(screen.getByRole('tree')).findByRole('button', { name: /^Second page/ }),
  );
  fireEvent.click(await screen.findByRole('button', { name: /^Channel: Second page/ }));
  const input = screen.getByLabelText('Channel name');
  fireEvent.change(input, { target: { value: 'Uncertain second-page draft' } });
  await act(async () => fireEvent.click(screen.getByRole('button', { name: 'Save channel' })));
  const original = writes[0]!;
  for (const nextStatus of [503, 200]) {
    status = nextStatus;
    await act(async () =>
      fireEvent.click(screen.getByRole('button', { name: 'Refresh channel status' })),
    );
    expect(input).toBeInTheDocument();
    expect(input).toHaveValue('Uncertain second-page draft');
    expect(screen.getByRole('button', { name: 'Retry same request' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Delete channel' })).toBeDisabled();
    await act(async () => fireEvent.submit(input.closest('form')!));
    expect(writes).toHaveLength(1);
  }
  fireEvent.click(screen.getByRole('button', { name: 'Close' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Load more channels' }));
  fireEvent.click(
    await within(screen.getByRole('tree')).findByRole('button', { name: /^Second page/ }),
  );
  fireEvent.click(await screen.findByRole('button', { name: /^Channel: Second page/ }));
  expect(screen.getByLabelText('Channel name')).toBe(input);
  expect(input).toHaveValue('Uncertain second-page draft');
  expect(screen.getByRole('button', { name: 'Retry same request' })).toBeEnabled();
  expect(writes).toHaveLength(1);
  await act(async () =>
    fireEvent.click(screen.getByRole('button', { name: 'Retry same request' })),
  );
  expect(writes).toHaveLength(2);
  expect(writes[1]?.path).toBe(original.path);
  expect(writes[1]?.request).toMatchObject({
    method: original.request.method,
    body: original.request.body,
    headers: original.request.headers,
  });
  expect(new Headers(original.request.headers).get('Idempotency-Key')).toBeTruthy();
});

it('treats a public owner-kind change as an identity mismatch even with an equal subject', () => {
  const previous = record('one', 'Channel');
  const next: Channel = {
    ...previous,
    version: 8,
    definition: { ...previous.definition, owner: { kind: 'organization', subject: 'owner' } },
  };
  const merged = mergeChannelEvidence(mergeChannelEvidence(undefined, previous), next, true);
  expect(channelResponseMatches(previous, next)).toBe(false);
  expect(channelIsWritable(merged, false)).toBe(false);
});
