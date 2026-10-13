import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { EventGateway } from './EventGateway';

afterEach(() => vi.unstubAllGlobals());
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
