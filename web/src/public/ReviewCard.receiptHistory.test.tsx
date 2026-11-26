import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError, createClient, TransportError } from '@cma/client';
import { PUBLIC_RECORD, PUBLIC_TRUST, publicClient } from '../test/public';
import { ReviewCard } from './ReviewCard';
import '../test/connectedAccounts';

beforeEach(() => {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input) => {
      if (input !== '/auth/session') throw new Error('Unexpected external fetch');
      return new Response(
        JSON.stringify({
          authenticated: true,
          subject: 'owner',
          visitor: true,
          capabilities: ['launch_triggers'],
        }),
        { headers: { 'Content-Type': 'application/json' } },
      );
    }),
  );
});
afterEach(() => {
  expect(vi.mocked(fetch).mock.calls.every(([input]) => input === '/auth/session')).toBe(true);
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  sessionStorage.clear();
});

async function begin(client: ReturnType<typeof publicClient>) {
  const actor = userEvent.setup();
  render(
    <ReviewCard
      expectedSubject="owner"
      client={client}
      record={PUBLIC_TRUST}
      onChanged={vi.fn()}
      onClose={vi.fn()}
    />,
  );
  await actor.click(screen.getByRole('checkbox'));
  const submit = await screen.findByRole('button', { name: 'Continue' });
  await waitFor(() => expect(submit).toBeEnabled());
  await actor.click(submit);
  return actor;
}

it.each([
  ['transport', 401],
  ['transport', 403],
  ['idempotency_recovery_required', 401],
  ['idempotency_recovery_required', 403],
] as const)(
  'retains the original Start key after %s then HTTP %i refusal',
  async (first, status) => {
    const client = publicClient();
    vi.mocked(client.requests.start)
      .mockRejectedValueOnce(
        first === 'transport'
          ? new Error('Synthetic response loss')
          : new ApiError(409, 'Original outcome unresolved', first),
      )
      .mockRejectedValueOnce(new ApiError(status, 'Synthetic access refusal', 'access_refused'))
      .mockResolvedValueOnce(PUBLIC_RECORD);
    const actor = await begin(client);
    await actor.click(await screen.findByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(client.requests.start).toHaveBeenCalledTimes(2));
    const retry = await screen.findByRole('button', { name: /^(Try again|Continue)$/ });
    await waitFor(() => expect(retry).toBeEnabled());
    await actor.click(retry);
    await waitFor(() => expect(client.requests.start).toHaveBeenCalledTimes(3));
    const calls = vi.mocked(client.requests.start).mock.calls;
    expect(calls[1]).toEqual(calls[0]);
    expect(calls[2]?.[0]).toBe(calls[0]?.[0]);
    expect(calls[2]?.[1]).toEqual(calls[0]?.[1]);
    expect(calls[2]?.[2].idempotencyKey).toBe(calls[0]?.[2].idempotencyKey);
    expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
  },
);

it('keeps the original trust selection locked after an unresolved Start retry is refused', async () => {
  const client = publicClient();
  vi.mocked(client.requests.start)
    .mockRejectedValueOnce(new Error('Synthetic response loss'))
    .mockRejectedValueOnce(new ApiError(403, 'Synthetic access refusal', 'access_refused'));
  const actor = await begin(client);
  await actor.click(await screen.findByRole('button', { name: 'Try again' }));
  await waitFor(() => expect(client.requests.start).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Try again' })).toBeEnabled());
  expect(screen.getByRole('checkbox')).toBeDisabled();
});

it('control: unknown then success retains the original command', async () => {
  const client = publicClient();
  vi.mocked(client.requests.start)
    .mockRejectedValueOnce(new Error('Synthetic response loss'))
    .mockResolvedValueOnce(PUBLIC_RECORD);
  const actor = await begin(client);
  await actor.click(await screen.findByRole('button', { name: 'Try again' }));
  await waitFor(() => expect(client.requests.start).toHaveBeenCalledTimes(2));
  const calls = vi.mocked(client.requests.start).mock.calls;
  expect(calls[1]).toEqual(calls[0]);
});

it('checks the Launch on RecoveryRequired without replacing the unresolved command', async () => {
  const client = publicClient();
  vi.mocked(client.requests.start)
    .mockRejectedValueOnce(new Error('Synthetic response loss'))
    .mockRejectedValueOnce(
      new ApiError(409, 'Original outcome unresolved', 'idempotency_recovery_required'),
    )
    .mockResolvedValueOnce(PUBLIC_RECORD);
  vi.mocked(client.requests.read).mockResolvedValueOnce(PUBLIC_TRUST);
  const actor = await begin(client);
  await actor.click(await screen.findByRole('button', { name: 'Try again' }));
  await waitFor(() => expect(client.requests.read).toHaveBeenCalledWith(PUBLIC_TRUST.id));
  expect(screen.getByRole('checkbox')).toBeDisabled();
  expect(client.requests.start).toHaveBeenCalledTimes(2);
  await actor.click(screen.getByRole('button', { name: 'Try again' }));
  await waitFor(() => expect(client.requests.start).toHaveBeenCalledTimes(3));
  const calls = vi.mocked(client.requests.start).mock.calls;
  expect(calls[2]).toEqual(calls[0]);
  expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
});

it('control: an initial definite refusal permits a fresh explicit command', async () => {
  const client = publicClient();
  vi.mocked(client.requests.start)
    .mockRejectedValueOnce(new ApiError(403, 'Synthetic initial refusal', 'access_refused'))
    .mockResolvedValueOnce(PUBLIC_RECORD);
  const actor = await begin(client);
  await waitFor(() => expect(client.requests.start).toHaveBeenCalledTimes(1));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Continue' })).toBeEnabled());
  await actor.click(screen.getByRole('button', { name: 'Continue' }));
  await waitFor(() => expect(client.requests.start).toHaveBeenCalledTimes(2));
  const calls = vi.mocked(client.requests.start).mock.calls;
  expect(calls[1]?.[2].idempotencyKey).not.toBe(calls[0]?.[2].idempotencyKey);
});

it('control: unknown then HTTP 429 retains the original command through recovery', async () => {
  const client = publicClient();
  vi.mocked(client.requests.start)
    .mockRejectedValueOnce(new Error('Synthetic response loss'))
    .mockRejectedValueOnce(new ApiError(429, 'Synthetic temporary refusal', 'rate_limited'))
    .mockResolvedValueOnce(PUBLIC_RECORD);
  const actor = await begin(client);
  await actor.click(await screen.findByRole('button', { name: 'Try again' }));
  await waitFor(() => expect(client.requests.start).toHaveBeenCalledTimes(2));
  const retry = screen.getByRole('button', { name: 'Try again' });
  await waitFor(() => expect(retry).toBeEnabled());
  await actor.click(retry);
  await waitFor(() => expect(client.requests.start).toHaveBeenCalledTimes(3));
  const calls = vi.mocked(client.requests.start).mock.calls;
  expect(calls[1]).toEqual(calls[0]);
  expect(calls[2]).toEqual(calls[0]);
});

it.each([401, 403])(
  'real SDK keeps the original HTTP Idempotency-Key after response loss then HTTP %i',
  async (status) => {
    const sent: { url: string; method: string; body: unknown; key: string | null }[] = [];
    const transport = vi.fn<typeof fetch>(async (input, init) => {
      const request = new Request(input, init);
      sent.push({
        url: request.url,
        method: request.method,
        body: await request.json(),
        key: request.headers.get('Idempotency-Key'),
      });
      if (sent.length === 1) throw new TypeError('Synthetic response loss');
      return new Response(
        JSON.stringify(
          sent.length === 2
            ? {
                status,
                title: 'Synthetic access refusal',
                code: status === 401 ? 'not_authenticated' : 'forbidden',
              }
            : PUBLIC_RECORD,
        ),
        {
          status: sent.length === 2 ? status : 202,
          headers: { 'Content-Type': 'application/json' },
        },
      );
    });
    const sdk = createClient({
      baseUrl: 'https://api.example.test',
      fetch: transport,
      requestIdProvider: () => 'reviewcard-retry-history',
    });
    const client = publicClient();
    vi.mocked(client.requests.start).mockImplementation((id, body, options) =>
      sdk.launches.start(id, body, options),
    );
    const actor = await begin(client);
    await actor.click(await screen.findByRole('button', { name: 'Try again' }));
    await waitFor(() => expect(transport).toHaveBeenCalledTimes(2));
    const retry = await screen.findByRole('button', { name: /^(Try again|Continue)$/ });
    await waitFor(() => expect(retry).toBeEnabled());
    await actor.click(retry);
    await waitFor(() => expect(transport).toHaveBeenCalledTimes(3));
    await waitFor(() =>
      expect(vi.mocked(client.requests.start).mock.settledResults[2]?.type).toBe('fulfilled'),
    );
    expect(vi.mocked(client.requests.start).mock.settledResults[0]?.value).toBeInstanceOf(
      TransportError,
    );
    expect(vi.mocked(client.requests.start).mock.settledResults[1]).toMatchObject({
      type: 'rejected',
      value: expect.objectContaining({
        status,
        code: status === 401 ? 'not_authenticated' : 'forbidden',
      }),
    });
    expect(
      sent.every(
        (item) => item.url === `https://api.example.test/api/v1/launches/${PUBLIC_TRUST.id}/start`,
      ),
    ).toBe(true);
    expect(sent.every((item) => item.method === 'POST')).toBe(true);
    expect(sent[1]).toEqual(sent[0]);
    expect(sent[2]?.body).toEqual(sent[0]?.body);
    expect(sent[0]?.key).toBeTruthy();
    expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
    expect(sent[2]?.key).toBe(sent[0]?.key);
  },
);
