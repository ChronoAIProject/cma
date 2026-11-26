import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor, type Channel } from '../ChannelEditor';
import { defaultProfile } from './model';

afterEach(() => vi.unstubAllGlobals());

const channel: Channel = {
  channel_id: 'channel-fixture',
  record_id: '11111111-1111-4111-8111-111111111111',
  version: 1,
  provider: { slug: 'cma_codex', level: 'l1' },
  endpoints: { custom_event_url: '', nyxid_callback_url: '' },
  definition: {
    name: 'Existing',
    profile: defaultProfile('owner'),
    provider: { slug: 'cma_codex' },
    sources: [
      { type: 'custom', auth: { type: 'nyxid_key', key_hashes: [`sha256:${'a'.repeat(64)}`] } },
    ],
    partition: { version: 1, key: 'per_event' },
    reply: { auto_final: true, ack_after_ms: 1500, sinks: [{ type: 'source' }] },
  },
};

it.each([
  ['channel_creation_pending: Complete the original channel creation, then refresh.', true],
  ['expected version 1, actual Some(2)', false],
])('preserves the draft after the version-one refusal: %s', async (detail, editable) => {
  const fetcher = vi
    .fn()
    .mockResolvedValue(new Response(JSON.stringify({ code: 'conflict', detail }), { status: 409 }));
  vi.stubGlobal('fetch', fetcher);
  render(<ChannelEditor channel={channel} onDone={vi.fn()} denied={vi.fn()} />);
  const name = screen.getByLabelText('Channel name');
  fireEvent.change(name, { target: { value: 'Retained draft' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save channel' }));
  await screen.findByText(detail);
  expect(name).toHaveValue('Retained draft');
  if (editable) {
    expect(name).toBeEnabled();
    expect(screen.queryByRole('button', { name: 'Retry same request' })).toBeNull();
  } else {
    expect(name).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Retry same request' })).toBeInTheDocument();
  }
  expect(fetcher).toHaveBeenCalledOnce();
});

it('retains the original request after uncertainty even if a retry reports pending creation', async () => {
  const detail = 'channel_creation_pending: Complete the original channel creation, then refresh.';
  const fetcher = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('Unconfirmed transport'))
    .mockResolvedValueOnce(
      new Response(JSON.stringify({ code: 'conflict', detail }), { status: 409 }),
    );
  vi.stubGlobal('fetch', fetcher);
  render(<ChannelEditor channel={channel} onDone={vi.fn()} denied={vi.fn()} />);
  fireEvent.click(screen.getByRole('button', { name: 'Save channel' }));
  await screen.findByText('Unconfirmed transport');
  fireEvent.click(screen.getByRole('button', { name: 'Retry same request' }));
  await screen.findByText(detail);
  expect(screen.getByLabelText('Channel name')).toBeDisabled();
  expect(screen.getByRole('button', { name: 'Retry same request' })).toBeInTheDocument();
  for (const key of ['method', 'body', 'headers'])
    expect(fetcher.mock.calls[1]![1][key]).toEqual(fetcher.mock.calls[0]![1][key]);
});
