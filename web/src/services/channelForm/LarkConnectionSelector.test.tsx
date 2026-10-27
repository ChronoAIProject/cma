import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { RelayCapabilitiesContext, type RelayDiscovery } from '../RelayCapabilities';
import { LarkResourceFields } from './LarkResourceFields';

const capability: RelayDiscovery = {
  status: 'ready',
  authoring: {
    lark_resource_writes_enabled: true,
    dynamic_sources_writes_enabled: false,
    multi_consumer_writes_enabled: false,
    relay_raw_verification_writes_enabled: false,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [],
  },
};
const binding = {
  user_service_id: 'saved-connection',
  catalog_service_id: 'saved-catalog',
  app_id: 'cli_independent',
};
const value = { platform: 'lark', bot_id: 'ou_independent', lark_resources: binding };
const connection = {
  user_service_id: 'selected-connection',
  catalog_service_id: 'selected-catalog',
  name: 'Research bot',
  source: 'organization',
  platform: 'lark',
};
const response = (items: unknown[] = [connection]) =>
  new Response(JSON.stringify({ items }), { status: 200 });
const mount = (onChange = vi.fn(), next = value, owner = 'alice') => (
  <RelayCapabilitiesContext key={owner} value={capability}>
    <LarkResourceFields value={next} onChange={onChange} />
  </RelayCapabilitiesContext>
);
afterEach(() => vi.unstubAllGlobals());

it('loads only on explicit request, selects both exact IDs, and preserves independent app/bot identity', async () => {
  const fetch = vi.fn().mockResolvedValue(response());
  vi.stubGlobal('fetch', fetch);
  const onChange = vi.fn();
  const view = render(mount(onChange));
  expect(fetch).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Load my bot connections' }));
  await screen.findByRole('button', { name: 'Existing bot API connection' });
  expect(fetch).toHaveBeenCalledWith(
    '/api/event-gateway/v1/relay-resource-connections?platform=lark',
    expect.objectContaining({ credentials: 'same-origin', cache: 'no-store' }),
  );
  expect(onChange).not.toHaveBeenCalled();
  expect(screen.getByLabelText('NyxID service connection ID')).toHaveValue('saved-connection');
  fireEvent.click(screen.getByRole('button', { name: 'Existing bot API connection' }));
  fireEvent.click(
    screen.getByRole('menuitemradio', {
      name: 'Research bot · Organization · selected-connection',
    }),
  );
  const updated = {
    ...value,
    lark_resources: {
      ...binding,
      user_service_id: connection.user_service_id,
      catalog_service_id: connection.catalog_service_id,
    },
  };
  expect(onChange).toHaveBeenCalledExactlyOnceWith(updated);
  view.rerender(mount(onChange, updated));
  expect(screen.getByLabelText('Receiving app ID')).toHaveValue('cli_independent');
  expect(screen.queryByLabelText('NyxID service connection ID')).not.toBeInTheDocument();
  fireEvent.click(screen.getByLabelText('Enter connection IDs manually'));
  expect(screen.getByLabelText('NyxID service connection ID')).toHaveValue(
    connection.user_service_id,
  );
  expect(fetch).toHaveBeenCalledTimes(1);
});

it.each([404, 403, 401, 503])(
  'retains saved/manual bindings after discovery failure %s and supports explicit retry',
  async (status) => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(new Response('{}', { status }))
      .mockResolvedValueOnce(response());
    vi.stubGlobal('fetch', fetch);
    const onChange = vi.fn();
    render(mount(onChange));
    fireEvent.click(screen.getByRole('button', { name: 'Load my bot connections' }));
    await screen.findByRole('button', { name: 'Retry connections' });
    expect(onChange).not.toHaveBeenCalled();
    expect(screen.getByLabelText('NyxID service connection ID')).toHaveValue(
      binding.user_service_id,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Retry connections' }));
    await screen.findByRole('button', { name: 'Existing bot API connection' });
    expect(fetch).toHaveBeenCalledTimes(2);
  },
);

it('does not reinterpret an empty inventory as deletion or permission revocation', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(response([])));
  const onChange = vi.fn();
  render(mount(onChange));
  fireEvent.click(screen.getByRole('button', { name: 'Load my bot connections' }));
  await screen.findByText(/No eligible Lark bot connections/);
  expect(screen.getByLabelText('NyxID catalog service ID')).toHaveValue(binding.catalog_service_id);
  expect(onChange).not.toHaveBeenCalled();
});

it.each([
  { items: [{ ...connection, platform: 'feishu' }] },
  { items: [connection, connection] },
  { items: [{ ...connection, user_service_id: '' }] },
  { items: [null] },
  { items: [{ ...connection, source: 'untrusted' }] },
])('refuses an inconsistent inventory without changing the current binding', async ({ items }) => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(response(items)));
  const onChange = vi.fn();
  render(mount(onChange));
  fireEvent.click(screen.getByRole('button', { name: 'Load my bot connections' }));
  await screen.findByRole('button', { name: 'Retry connections' });
  expect(onChange).not.toHaveBeenCalled();
  expect(
    screen.queryByRole('button', { name: 'Existing bot API connection' }),
  ).not.toBeInTheDocument();
});

it.each(['region', 'owner', 'permission'])(
  'discards late discovery after %s changes',
  async (change) => {
    let finish!: (response: Response) => void;
    const fetch = vi.fn().mockImplementation(
      () =>
        new Promise<Response>((resolve) => {
          finish = resolve;
        }),
    );
    vi.stubGlobal('fetch', fetch);
    const onChange = vi.fn();
    const view = render(mount(onChange));
    fireEvent.click(screen.getByRole('button', { name: 'Load my bot connections' }));
    if (change === 'region') view.rerender(mount(onChange, { ...value, platform: 'feishu' }));
    else if (change === 'owner') view.rerender(mount(onChange, value, 'bob'));
    else view.rerender(<LarkResourceFields value={value} onChange={onChange} />);
    await act(async () => finish(response()));
    expect(
      screen.queryByRole('button', { name: 'Existing bot API connection' }),
    ).not.toBeInTheDocument();
    expect(onChange).not.toHaveBeenCalled();
    expect(fetch.mock.calls[0]?.[1]?.signal.aborted).toBe(true);
    if (change === 'region') {
      fetch.mockResolvedValueOnce(response([{ ...connection, platform: 'feishu' }]));
      fireEvent.click(screen.getByRole('button', { name: 'Load my bot connections' }));
      await waitFor(() =>
        expect(fetch).toHaveBeenLastCalledWith(
          '/api/event-gateway/v1/relay-resource-connections?platform=feishu',
          expect.anything(),
        ),
      );
    }
  },
);

it('keeps legacy capability-off records read-only and makes no discovery call', () => {
  const fetch = vi.fn();
  vi.stubGlobal('fetch', fetch);
  const onChange = vi.fn();
  render(<LarkResourceFields value={value} onChange={onChange} />);
  expect(screen.queryByRole('button', { name: 'Load my bot connections' })).not.toBeInTheDocument();
  expect(screen.getByLabelText('NyxID service connection ID')).toBeDisabled();
  fireEvent.click(screen.getByLabelText('Read media in rich-text posts'));
  expect(onChange).toHaveBeenCalledWith({ platform: value.platform, bot_id: value.bot_id });
  expect(fetch).not.toHaveBeenCalled();
});
