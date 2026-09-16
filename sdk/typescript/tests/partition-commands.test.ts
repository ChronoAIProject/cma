import { expect, it, vi } from 'vitest';
import { createClient, ProtocolError, TransportError, ValidationError } from '../src/index.js';
import { bridge, allocation, receipt, request } from './partition-commands-fixtures.js';
const response = (body: unknown) =>
  new Response(JSON.stringify(body), {
    status: 202,
    headers: { 'content-type': 'application/json' },
  });

it('captures the exact generation and request key before credentials and replays only on explicit call', async () => {
  let release!: (value: string) => void;
  const credential = new Promise<string>((done) => {
    release = done;
  });
  const fetch = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('reply lost'))
    .mockResolvedValueOnce(response(receipt()));
  const client = createClient({
    baseUrl: 'https://fixture.test',
    credential: () => credential,
    fetch,
  });
  const body = { ...request };
  const options = { idempotencyKey: 'original-request' };
  const pending = client.bridges.resetPartition(bridge, allocation, body, options);
  body.expected_revision = 90;
  body.expected_bridge_revision = 91;
  options.idempotencyKey = 'replacement';
  release('credential');
  await expect(pending).rejects.toBeInstanceOf(TransportError);
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(new URL(fetch.mock.calls[0]![0]).pathname).toBe(
    `/api/v1/bridges/${bridge}/partitions/${allocation}/reset`,
  );
  expect(JSON.parse(fetch.mock.calls[0]![1].body)).toEqual(request);
  expect(new Headers(fetch.mock.calls[0]![1].headers).get('idempotency-key')).toBe(
    'original-request',
  );
  expect(
    await client.bridges.resetPartition(bridge, allocation, request, {
      idempotencyKey: 'original-request',
    }),
  ).toEqual(receipt());
  expect(fetch).toHaveBeenCalledTimes(2);
});

it('requires exact reset and detach receipt identities and closed actor/result fields', async () => {
  const fetch = vi.fn();
  const client = createClient({
    baseUrl: 'https://fixture.test',
    credential: () => 'credential',
    fetch,
  });
  const reset = receipt();
  for (const malformed of [
    { ...reset, action: 'detach' },
    { ...reset, bridge_id: 'another' },
    { ...reset, previous_revision: 4 },
    { ...reset, successor: null },
    { ...reset, successor: { ...reset.successor!, generation: 3 } },
    { ...reset, successor: { ...reset.successor!, revision: 2 } },
    { ...reset, result: { ...reset.result, state: 'ready' } },
    { ...reset, actor: { kind: 'api', subject: 'owner', delegation_id: 'delegated' } },
    { ...reset, credential: 'private' },
  ]) {
    fetch.mockResolvedValueOnce(response(malformed));
    await expect(
      client.bridges.resetPartition(bridge, allocation, request, { idempotencyKey: 'original' }),
    ).rejects.toBeInstanceOf(ProtocolError);
  }
  fetch.mockResolvedValueOnce(response(receipt('detach')));
  expect(
    await client.bridges.detachPartition(bridge, allocation, request, {
      idempotencyKey: 'detach-original',
    }),
  ).toEqual(receipt('detach'));
  expect(new URL(fetch.mock.lastCall![0]).pathname.endsWith('/detach')).toBe(true);
});

it('rejects invalid scope, unknown request fields and missing revision before any credential or network work', () => {
  const credential = vi.fn(() => 'credential');
  const fetch = vi.fn();
  const client = createClient({ baseUrl: 'https://fixture.test', credential, fetch });
  for (const body of [
    { ...request, expected_revision: 0 },
    { ...request, action: 'reset' },
    { ...request, expected_bridge_revision: -1 },
  ]) {
    expect(() =>
      client.bridges.detachPartition(bridge, allocation, body, { idempotencyKey: 'original' }),
    ).toThrow(ValidationError);
  }
  expect(() =>
    client.bridges.resetPartition(bridge, 'wrong', request, { idempotencyKey: 'original' }),
  ).toThrow(ValidationError);
  expect(() =>
    client.bridges.resetPartition(bridge, allocation, request, { idempotencyKey: '' }),
  ).toThrow(ValidationError);
  expect(credential).not.toHaveBeenCalled();
  expect(fetch).not.toHaveBeenCalled();
});
