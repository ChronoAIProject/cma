// @vitest-environment node
import { expect, it, vi } from 'vitest';
import { ApiError, createClient, pendingRevocation, ProtocolError } from '../index.js';
import { revocationStatus } from '../resource-revocations.js';

const operation = 'a'.repeat(64);
const pending = {
  operation_id: operation,
  selector: { kind: 'agent', agent_id: 'agent' },
  share_id: 'share',
  expected_revision: 1,
  state: 'pending',
  participants: 2,
  stopped: 1,
  unresolved_effects: 1,
  deadline_unix_ms: 11000,
  deadline_exceeded: true,
  receipt: null,
};
const link = `</api/v1/resource-revocations/${operation}?kind=agent&id=agent>; rel="status"`;

it('requires 202 for creation, retains the original key and reads only the exact operation', async () => {
  const fetch = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) =>
    Response.json(pending, { status: init?.method === 'POST' ? 202 : 200 }),
  );
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const body = {
    kind: 'agent',
    id: 'agent',
    share_id: 'share',
    expected_revision: 1,
    change: { kind: 'revoke' },
  } as const;
  for (let i = 0; i < 2; i++)
    expect(await client.resourceRevocations.request(body, { idempotencyKey: 'original' })).toEqual(
      pending,
    );
  expect(
    fetch.mock.calls.map(([, init]) => new Headers(init?.headers).get('Idempotency-Key')),
  ).toEqual(['original', 'original']);
  expect(await client.resourceRevocations.status(operation, 'agent', 'agent')).toEqual(pending);
  await expect(
    client.resourceRevocations.status('b'.repeat(64), 'agent', 'agent'),
  ).rejects.toBeInstanceOf(ProtocolError);
  await expect(
    client.resourceRevocations.status(operation, 'agent', 'other'),
  ).rejects.toBeInstanceOf(ProtocolError);
  fetch.mockImplementation(async () => Response.json(pending));
  await expect(
    client.resourceRevocations.request(body, { idempotencyKey: 'original' }),
  ).rejects.toBeInstanceOf(ProtocolError);
});

it.each([
  { participants: 65 },
  { stopped: 3 },
  { unresolved_effects: 257 },
  { stopped: -1 },
  { state: 'effective' },
  { state: 'evidence_received' },
  { operation_id: '../other' },
  { receipt: {} },
])('refuses inconsistent evidence: %j', (patch) => {
  expect(() => revocationStatus({ ...pending, ...patch })).toThrow(ProtocolError);
});

it('keeps predecessor mutations incomplete and accepts only a same-resource status link', async () => {
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async () =>
      Response.json(
        { code: 'resource_revocation_pending', detail: 'Pending' },
        { status: 409, headers: { Link: link } },
      ),
  });
  const error = await client.resourceAccess
    .revokeShare('agent', 'agent', 'share', 1, { idempotencyKey: 'original' })
    .catch((value: unknown) => value);
  expect(error).toBeInstanceOf(ApiError);
  expect(pendingRevocation(error, 'agent', 'agent')).toBe(operation);
  expect(pendingRevocation(error, 'agent', 'other')).toBeUndefined();
  expect(pendingRevocation(error, 'sandbox', 'agent')).toBeUndefined();
  for (const statusLink of [
    link.replace('</', '<https://foreign.invalid/'),
    link + ', <https://foreign.invalid>',
    link.replace('?kind', '?extra=x&kind'),
    link.replace('id=agent', 'id=agent&id=other'),
    link.replace('status', 'next'),
  ]) {
    expect(
      pendingRevocation(
        new ApiError(
          409,
          'Pending',
          'resource_revocation_pending',
          undefined,
          undefined,
          {},
          statusLink,
        ),
        'agent',
        'agent',
      ),
    ).toBeUndefined();
  }
});

it('rejects an effective creation receipt for a different share change', async () => {
  const actor = { kind: 'human', subject: 'owner' };
  const receipt = {
    operation_id: operation,
    share: {
      id: 'share',
      owner: { kind: 'person', subject: 'owner' },
      selector: pending.selector,
      target_org_id: 'org',
      mode: 'view',
      issuer_actor: actor,
      created_at: '2026-01-01T00:00:00Z',
      revoked_at: null,
      revision: 2,
    },
    actor,
    at: '2026-01-01T00:00:00Z',
    policy_revision: 2,
  };
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async () =>
      Response.json({ ...pending, state: 'effective', stopped: 2, receipt }, { status: 202 }),
  });
  await expect(
    client.resourceRevocations.request(
      {
        kind: 'agent',
        id: 'agent',
        share_id: 'share',
        expected_revision: 1,
        change: { kind: 'revoke' },
      },
      { idempotencyKey: 'original' },
    ),
  ).rejects.toBeInstanceOf(ProtocolError);
  await expect(
    client.resourceRevocations.request(
      {
        kind: 'agent',
        id: 'agent',
        share_id: 'share',
        expected_revision: 1,
        change: { kind: 'set_mode', mode: 'view_write' },
      },
      { idempotencyKey: 'original' },
    ),
  ).rejects.toBeInstanceOf(ProtocolError);
});
