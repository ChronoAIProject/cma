import { expect, it, vi } from 'vitest';
import { createClient, ProtocolError, TransportError, ValidationError } from '../src/index.js';
import { bridge, allocation, request, response, row, status } from './bridge-cleanup-fixtures.js';

it('captures exact cleanup consent and key before credentials and recovers only by explicit replay', async () => {
  let release!: (credential: string) => void;
  const credentials = new Promise<string>((resolve) => {
    release = resolve;
  });
  const fetch = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('reply lost'))
    .mockResolvedValueOnce(response(status()));
  const client = createClient({
    baseUrl: 'https://fixture.test',
    credential: () => credentials,
    fetch,
  });
  const body = { ...request };
  const options = { idempotencyKey: 'original-cleanup' };
  const pending = client.bridges.requestChildCleanup(bridge, allocation, body, options);
  body.expected_bridge_revision = 99;
  body.expected_allocation_revision = 100;
  options.idempotencyKey = 'replacement-key';
  release('selected-credential');
  await expect(pending).rejects.toBeInstanceOf(TransportError);
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(JSON.parse(fetch.mock.calls[0]![1].body)).toEqual(request);
  expect(new Headers(fetch.mock.calls[0]![1].headers).get('idempotency-key')).toBe(
    'original-cleanup',
  );
  const accepted = await client.bridges.requestChildCleanup(bridge, allocation, request, {
    idempotencyKey: 'original-cleanup',
  });
  expect(accepted).toEqual(status());
  expect(accepted.observed_at_ms).toBeLessThan(accepted.receipt!.accepted_at_ms);
  expect(fetch).toHaveBeenCalledTimes(2);
  expect(JSON.parse(fetch.mock.calls[1]![1].body)).toEqual(request);
  expect(new Headers(fetch.mock.calls[1]![1].headers).get('idempotency-key')).toBe(
    'original-cleanup',
  );
});

it('preserves opaque allocation cursors and validates exact scope, ordered rows and filtered continuation', async () => {
  const fetch = vi.fn();
  const client = createClient({
    baseUrl: 'https://fixture.test',
    credential: () => 'selected-credential',
    fetch,
  });
  const opaque = 'opaque-page/+/v1?part=1';
  const page = { items: [row()], next_cursor: opaque };
  fetch.mockResolvedValueOnce(response(page, 200));
  expect(await client.bridges.listAllocations(bridge, { limit: 1 })).toEqual(page);
  fetch.mockResolvedValueOnce(response({ items: [], next_cursor: null }, 200));
  await client.bridges.listAllocations(bridge, { cursor: opaque, limit: 1 });
  expect(new URL(fetch.mock.calls[1]![0]).searchParams.get('cursor')).toBe(opaque);
  for (const items of [[], [row()]]) {
    const continued = { items, next_cursor: 'another-page' };
    fetch.mockResolvedValueOnce(response(continued, 200));
    expect(await client.bridges.listAllocations(bridge, { cursor: opaque, limit: 2 })).toEqual(
      continued,
    );
  }
  for (const malformed of [
    { items: [row()], next_cursor: opaque },
    { items: [], next_cursor: '' },
    { items: [], next_cursor: 'x'.repeat(1025) },
    { items: [row(), row()], next_cursor: null },
    { items: [{ ...row(), bridge_id: 'another' }], next_cursor: null },
    { items: [{ ...row(), generation: 257 }], next_cursor: null },
    { items: [{ ...row(), target: { ...row().target, credential: 'hidden' } }], next_cursor: null },
  ]) {
    fetch.mockResolvedValueOnce(response(malformed, 200));
    await expect(
      client.bridges.listAllocations(bridge, { cursor: opaque, limit: 1 }),
    ).rejects.toBeInstanceOf(ProtocolError);
  }
});

it('retains exact target and receipt while progress reaches Complete and refuses contradictory proof', async () => {
  const fetch = vi.fn();
  const client = createClient({
    baseUrl: 'https://fixture.test',
    credential: () => 'selected-credential',
    fetch,
  });
  const done = { ...status(), progress: 'complete', holds: [], agent_capacity_released: true };
  fetch.mockResolvedValueOnce(response(done, 200));
  expect(await client.bridges.readChildCleanup(bridge, allocation)).toEqual(done);
  fetch.mockResolvedValueOnce(response(status(), 200));
  await expect(
    client.bridges.requestChildCleanup(bridge, allocation, request, {
      idempotencyKey: 'same-cleanup',
    }),
  ).rejects.toBeInstanceOf(ProtocolError);
  const before = {
    ...status(),
    receipt: null,
    progress: 'not_requested',
    holds: [],
    observed_at_ms: null,
  };
  fetch.mockResolvedValueOnce(response(before, 200));
  expect(await client.bridges.readChildCleanup(bridge, allocation)).toEqual(before);
  for (const bad of [
    { ...status(), bridge_id: 'another' },
    { ...status(), allocation_id: `bral_${'3'.repeat(64)}` },
    { ...status(), target: { ...status().target, agent_id: 'replacement' } },
    { ...status(), receipt: { ...status().receipt, generation: 2 } },
    {
      ...status(),
      receipt: { ...status().receipt, request: { ...request, expected_bridge_revision: 8 } },
    },
    {
      ...status(),
      receipt: {
        ...status().receipt,
        requested_by: { kind: 'api', subject: 'manager', delegation_id: 'delegated' },
      },
    },
    {
      ...status(),
      receipt: {
        ...status().receipt,
        requested_by: { kind: 'human', subject: 'manager', credential: 'hidden' },
      },
    },
    { ...status(), holds: ['uncertain_reply', 'pending_command'] },
    { ...status(), holds: ['uncertain_reply', 'uncertain_reply'] },
    { ...status(), progress: 'complete' },
    { ...done, holds: ['uncertain_reply'] },
    { ...done, history_retained: false },
    { ...done, workspace_retained: false },
    { ...status(), observed_at_ms: null },
    before,
    { ...status(), private_evidence: {} },
  ]) {
    fetch.mockResolvedValueOnce(response(bad));
    await expect(
      client.bridges.requestChildCleanup(bridge, allocation, request, {
        idempotencyKey: 'same-cleanup',
      }),
    ).rejects.toBeInstanceOf(ProtocolError);
  }
});

it('refuses invalid keys, revisions, targeting fields and limits before credential lookup or HTTP', () => {
  const token = vi.fn(() => 'selected-credential');
  const fetch = vi.fn();
  const client = createClient({ baseUrl: 'https://fixture.test', credential: token, fetch });
  for (const key of ['', 'x'.repeat(201), 'contains space', 'line\nbreak'])
    expect(() =>
      client.bridges.requestChildCleanup(bridge, allocation, request, { idempotencyKey: key }),
    ).toThrow(ValidationError);
  for (const body of [
    { ...request, action: 'remove_workspace' },
    { ...request, expected_bridge_revision: 0 },
    { ...request, expected_allocation_revision: Number.MAX_SAFE_INTEGER + 1 },
    { ...request, target: status().target },
  ])
    expect(() =>
      client.bridges.requestChildCleanup(bridge, allocation, body as never, {
        idempotencyKey: 'retained',
      }),
    ).toThrow(ValidationError);
  for (const options of [
    { limit: 0 },
    { limit: 101 },
    { cursor: '' },
    { cursor: 'x'.repeat(1025) },
  ])
    expect(() => client.bridges.listAllocations(bridge, options)).toThrow(ValidationError);
  expect(token).not.toHaveBeenCalled();
  expect(fetch).not.toHaveBeenCalled();
});
