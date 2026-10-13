import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { EventGateway } from './EventGateway';
import { ChannelEditor, type Channel } from './ChannelEditor';

afterEach(() => vi.unstubAllGlobals());
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
    const input = screen.getByLabelText<HTMLTextAreaElement>('Channel configuration JSON');
    const policy = JSON.parse(input.value);
    policy.name = 'Renamed channel';
    fireEvent.change(input, { target: { value: JSON.stringify(policy) } });
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

it('keeps legacy channel metadata visible and uses its existing console while modern rows retain actions', async () => {
  const fetcher = vi.fn().mockResolvedValue(
    new Response(
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
            definition: { name: 'New channel', sources: [] },
            endpoints: {
              custom_event_url: 'https://example.test/event',
              nyxid_callback_url: 'https://example.test/callback',
            },
          },
        ],
      }),
    ),
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
  const legacy = (await screen.findByRole('heading', { name: 'Existing channel' })).closest('li')!;
  expect(within(legacy).getByRole('link', { name: 'Manage existing channel' })).toHaveAttribute(
    'href',
    'https://bot-gateway.chrono-ai.fun/console/channels/legacy-record',
  );
  expect(within(legacy).queryByRole('button', { name: 'Delete channel' })).not.toBeInTheDocument();
  expect(within(legacy).queryByRole('button', { name: 'Send test event' })).not.toBeInTheDocument();
  expect(within(legacy).queryByText('Edit channel')).not.toBeInTheDocument();
  await userEvent.click(within(legacy).getByText('Channel details'));
  expect(within(legacy).getByText(/view its endpoints and manage it/)).toBeVisible();
  const modern = screen.getByRole('heading', { name: 'New channel' }).closest('li')!;
  expect(within(modern).getByRole('button', { name: 'Delete channel' })).toBeEnabled();
  expect(within(modern).getByRole('button', { name: 'Send test event' })).toBeEnabled();
  expect(fetcher).toHaveBeenCalledOnce();
});
