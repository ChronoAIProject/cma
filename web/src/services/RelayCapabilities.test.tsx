import { StrictMode } from 'react';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { RelayCapabilitiesProvider, useRelayCapabilities } from './RelayCapabilities';
const response = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status });
afterEach(() => vi.unstubAllGlobals());
function Probe() {
  const value = useRelayCapabilities();
  return (
    <>
      <span>{value.status}</span>
      <output>
        {value.managedTelegramSetup ? 'managed-setup-enabled' : 'managed-setup-disabled'}
      </output>
      <span>{value.authoring?.platforms[0]?.id}</span>
      <button onClick={value.retry}>Retry</button>
    </>
  );
}
function Portal({
  owner,
  denied = vi.fn(),
}: {
  owner: string;
  denied?: (status?: number) => void;
}) {
  return (
    <RelayCapabilitiesProvider key={owner} denied={denied}>
      <Probe />
      <Probe />
    </RelayCapabilitiesProvider>
  );
}
it('shares one request across editors, strict effects and navigation, resetting for another owner', async () => {
  const fetcher = vi
    .fn()
    .mockImplementation(async () =>
      response({ relay_authoring: { platforms: [{ id: 'discord' }] } }),
    );
  vi.stubGlobal('fetch', fetcher);
  const view = render(
    <StrictMode>
      <Portal owner="alice" />
    </StrictMode>,
  );
  expect(await screen.findAllByText('ready')).toHaveLength(2);
  view.rerender(
    <StrictMode>
      <Portal owner="alice" />
    </StrictMode>,
  );
  expect(fetcher).toHaveBeenCalledOnce();
  view.rerender(
    <StrictMode>
      <Portal owner="bob" />
    </StrictMode>,
  );
  await waitFor(() => expect(fetcher).toHaveBeenCalledTimes(2));
  expect(fetcher.mock.calls[0]?.[0]).toBe('/api/event-gateway/v1/channel-capabilities');
  expect(fetcher.mock.calls[0]?.[1]).toMatchObject({
    credentials: 'same-origin',
    cache: 'no-store',
  });
});
it.each([200, 404])(
  'keeps older gateway discovery usable when additive metadata is absent (%s)',
  async (status) => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockImplementation(async () => response({ platforms: [] }, status)),
    );
    render(<Portal owner="alice" />);
    expect(await screen.findAllByText('legacy')).toHaveLength(2);
  },
);
it.each([401, 403])(
  'reports authenticated discovery refusal %s without polling',
  async (status) => {
    const fetcher = vi.fn().mockImplementation(async () => response({}, status));
    vi.stubGlobal('fetch', fetcher);
    const denied = vi.fn();
    render(<Portal owner="alice" denied={denied} />);
    await waitFor(() => expect(denied).toHaveBeenCalledWith(status));
    expect(fetcher).toHaveBeenCalledOnce();
  },
);
it('retries a failed discovery only on explicit action', async () => {
  const fetcher = vi
    .fn()
    .mockRejectedValueOnce(new Error('offline'))
    .mockResolvedValueOnce(response({ platforms: [] }));
  vi.stubGlobal('fetch', fetcher);
  render(<Portal owner="alice" />);
  expect(await screen.findAllByText('failed')).toHaveLength(2);
  expect(fetcher).toHaveBeenCalledOnce();
  fireEvent.click(screen.getAllByRole('button', { name: 'Retry' })[0]!);
  expect(await screen.findAllByText('legacy')).toHaveLength(2);
  expect(fetcher).toHaveBeenCalledTimes(2);
});
it('does not show a previous owner response after identity changes', async () => {
  let complete!: (value: Response) => void;
  vi.stubGlobal(
    'fetch',
    vi
      .fn()
      .mockImplementationOnce(
        () =>
          new Promise<Response>((resolve) => {
            complete = resolve;
          }),
      )
      .mockResolvedValueOnce(response({ platforms: [] })),
  );
  const view = render(<Portal owner="alice" />);
  view.rerender(<Portal owner="bob" />);
  expect(await screen.findAllByText('legacy')).toHaveLength(2);
  await act(async () =>
    complete(response({ relay_authoring: { platforms: [{ id: 'old-owner' }] } })),
  );
  expect(screen.queryByText('old-owner')).not.toBeInTheDocument();
});

it.each([
  [{ platforms: [{ id: 'telegram', connection_methods: ['managed_create'] }] }, true],
  [{ platforms: [{ id: 'telegram', connection_methods: ['existing_credential'] }] }, false],
  [{ platforms: [] }, false],
  [{}, false],
])('uses explicit managed setup discovery (%j)', async (capabilities, enabled) => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(response(capabilities)));
  render(<Portal owner="alice" />);
  await screen.findAllByText('legacy');
  expect(
    screen.getAllByText(enabled ? 'managed-setup-enabled' : 'managed-setup-disabled'),
  ).toHaveLength(2);
});
