import { useState } from 'react';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ManagedConnectionFields } from './ManagedConnectionFields';
import { channelRequest, type Fields } from './model';
afterEach(() => vi.unstubAllGlobals());
const manual = {
  sources: [
    { type: 'nyxid_relay', platform: 'discord', verify_raw_event: true, route_ids: ['retained'] },
  ],
};
function Editor({
  inspect,
  denied,
}: {
  inspect: (value: Fields) => void;
  denied: (status?: number) => void;
}) {
  const [value, setValue] = useState<Fields>(manual);
  return (
    <ManagedConnectionFields
      value={value}
      denied={denied}
      onChange={(next) => {
        inspect(next);
        setValue(next);
      }}
    />
  );
}
it('loads only owned connections on demand, keeps admission closed and restores the exact manual draft', async () => {
  const fetcher = vi.fn().mockImplementation(
    async () =>
      new Response(
        JSON.stringify({
          items: [
            { connection_ref: 'ready-owned', status: 'ready', label: 'My Telegram bot' },
            {
              connection_ref: 'pending-owned',
              status: 'awaiting_platform',
              label: 'Unfinished bot',
            },
          ],
          next_after: null,
        }),
      ),
  );
  vi.stubGlobal('fetch', fetcher);
  const inspect = vi.fn();
  render(<Editor inspect={inspect} denied={vi.fn()} />);
  expect(fetcher).not.toHaveBeenCalled();
  const toggle = screen.getByLabelText('Use an existing CMA-managed Telegram connection');
  fireEvent.click(toggle);
  await waitFor(() => expect(fetcher).toHaveBeenCalledOnce());
  expect(inspect.mock.lastCall?.[0]).toMatchObject({
    sources: [],
    connection_admission: { type: 'allowlist', sender_ids: [] },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Saved Telegram connection' }));
  expect(await screen.findByRole('menuitemradio', { name: /Unfinished bot/ })).toHaveAttribute(
    'aria-disabled',
    'true',
  );
  fireEvent.click(screen.getByRole('menuitemradio', { name: 'My Telegram bot' }));
  expect(inspect.mock.lastCall?.[0]).toMatchObject({ connection_ref: 'ready-owned', sources: [] });
  fireEvent.click(toggle);
  expect(inspect.mock.lastCall?.[0]).toEqual(manual);
  fireEvent.click(toggle);
  expect(inspect.mock.lastCall?.[0]).toMatchObject({ connection_ref: 'ready-owned', sources: [] });
  expect(fetcher).toHaveBeenCalledOnce();
  expect(fetcher.mock.calls[0]?.[0]).toBe('/api/event-gateway/v1/channel-connections');
});
it('omits manual platform credentials only from the managed create request, retaining them in the draft', () => {
  const credentials = {
    agent_key: 'provider-fixture',
    channel_key: 'relay-fixture',
    telegram_bot_token: 'manual-fixture',
    telegram_webhook_secret: 'webhook-fixture',
  };
  const request = channelRequest(
    { sources: [], connection_ref: 'ready-owned' },
    credentials,
    undefined,
    'record',
  );
  expect(request.credentials).toEqual({ agent_key: 'provider-fixture' });
  expect(credentials.telegram_bot_token).toBe('manual-fixture');
  expect(() => channelRequest({ sources: [], connection_ref: '' }, credentials)).toThrow(
    /Choose a ready/,
  );
});
it('does not treat an ownership refusal as an empty list or broaden the owner', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response('{}', { status: 403 })));
  const denied = vi.fn();
  render(<Editor inspect={vi.fn()} denied={denied} />);
  fireEvent.click(screen.getByLabelText('Use an existing CMA-managed Telegram connection'));
  await waitFor(() => expect(denied).toHaveBeenCalledWith(403));
  expect(screen.getByRole('alert')).toBeVisible();
});
