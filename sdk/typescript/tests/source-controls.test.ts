import { expect, it, vi } from 'vitest';
import {
  ApiError,
  createClient,
  ProtocolError,
  TransportError,
  ValidationError,
  type BridgeSourceControlRequest,
} from '../src/index.js';
import { edit, receipt, response } from './source-controls-fixtures.js';

function fixture() {
  const fetch = vi.fn();
  const client = createClient({
    baseUrl: 'https://consent.test/proxy',
    credential: () => 'fixture-manager',
    fetch,
  });
  return { fetch, client, grant: receipt().result.grant_id, request: receipt().request };
}
it('preserves required null absence separately from explicit consent and later revocation', async () => {
  const { client, fetch, grant } = fixture();
  for (const policy of [
    null,
    receipt().result,
    { ...receipt().result, revision: 2, enabled: false },
    { ...receipt().result, revision: 256, enabled: false },
  ]) {
    fetch.mockResolvedValueOnce(response({ policy }));
    expect(await client.bridges.readSourceControls(grant)).toEqual({ policy });
  }
  expect(fetch).toHaveBeenCalledTimes(4);
  expect(new URL(fetch.mock.calls[0]![0]).pathname).toBe(
    `/proxy/api/v1/bridge-invoke-grants/${grant}/source-controls`,
  );
});
it('captures the original request and key before credentials and retries only on an explicit call', async () => {
  let release!: (value: string) => void;
  const credentials = new Promise<string>((resolve) => {
    release = resolve;
  });
  const fetch = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('reply lost'))
    .mockResolvedValueOnce(response(receipt()));
  const client = createClient({
    baseUrl: 'https://consent.test',
    credential: () => credentials,
    fetch,
  });
  const original = receipt();
  const body = { ...original.request };
  const options = { idempotencyKey: 'original-consent' };
  const pending = client.bridges.setSourceControls(original.result.grant_id, body, options);
  body.expected_bridge_revision = 99;
  body.expected_grant_revision = 2;
  body.expected_policy_revision = 9;
  body.action = 'revoke_sender_controls';
  options.idempotencyKey = 'replacement';
  release('fixture-manager');
  await expect(pending).rejects.toBeInstanceOf(TransportError);
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(JSON.parse(fetch.mock.calls[0]![1].body)).toEqual(original.request);
  expect(new Headers(fetch.mock.calls[0]![1].headers).get('idempotency-key')).toBe(
    'original-consent',
  );
  expect(
    await client.bridges.setSourceControls(original.result.grant_id, original.request, {
      idempotencyKey: 'original-consent',
    }),
  ).toEqual(original);
  expect(fetch).toHaveBeenCalledTimes(2);
  expect(fetch.mock.calls[1]![1].body).toBe(fetch.mock.calls[0]![1].body);
  expect(new Headers(fetch.mock.calls[1]![1].headers).get('idempotency-key')).toBe(
    'original-consent',
  );
});
it.each([409, 503])(
  'retains %i failure without automatic mutation retry and permits unchanged historical recovery',
  async (status) => {
    const { client, fetch, grant, request } = fixture();
    fetch.mockResolvedValueOnce(
      response(
        {
          type: 'about:blank',
          title: 'Unavailable',
          status,
          detail: 'Keep original request',
          code: 'consent_conflict',
        },
        status,
      ),
    );
    await expect(
      client.bridges.setSourceControls(grant, request, { idempotencyKey: 'same' }),
    ).rejects.toBeInstanceOf(ApiError);
    expect(fetch).toHaveBeenCalledTimes(1);
    fetch.mockResolvedValueOnce(
      response({ policy: { ...receipt().result, revision: 2, enabled: false } }),
    );
    expect((await client.bridges.readSourceControls(grant)).policy!.enabled).toBe(false);
    fetch.mockResolvedValueOnce(response(receipt()));
    expect(
      (await client.bridges.setSourceControls(grant, request, { idempotencyKey: 'same' })).result
        .enabled,
    ).toBe(true);
    expect(fetch).toHaveBeenCalledTimes(3);
    expect(fetch.mock.calls[2]![1].body).toBe(fetch.mock.calls[0]![1].body);
  },
);
it('refuses missing absence, wrong grant and contradictory or private policy fields', async () => {
  const { client, fetch, grant } = fixture();
  for (const state of [
    {},
    { policy: undefined },
    { policy: { ...receipt().result, grant_id: `brinv_${'a'.repeat(64)}` } },
    { policy: { ...receipt().result, enabled: true, revision: 256 } },
    { policy: { ...receipt().result, scope: 'all_senders' } },
    { policy: { ...receipt().result, approval_mode: 'channel' } },
    { policy: { ...receipt().result, private_evidence: {} } },
    { policy: { ...receipt().result, id: 'wrong' } },
    {
      policy: {
        ...receipt().result,
        updated_by: { kind: 'api', subject: 'owner-a', delegation_id: 'delegated' },
      },
    },
  ]) {
    fetch.mockResolvedValueOnce(response(state));
    await expect(client.bridges.readSourceControls(grant)).rejects.toBeInstanceOf(ProtocolError);
  }
});
it('refuses changed receipts, mixed request revisions and unsafe attribution', async () => {
  const { client, fetch, grant, request } = fixture();
  for (const [pointer, value] of [
    ['/id', 'bad'],
    ['/request/expected_bridge_revision', 99],
    ['/request/expected_grant_revision', 2],
    ['/request/expected_policy_revision', 1],
    ['/request/action', 'revoke_sender_controls'],
    ['/result/revision', 2],
    ['/result/enabled', false],
    ['/result/grant_id', `brinv_${'a'.repeat(64)}`],
    ['/result/pin/config_revision', 257],
    ['/result/target/agent_creation_fingerprint', 'bad'],
    ['/result/updated_at', ''],
    ['/result/updated_by/subject', ''],
    ['/result/target/ids/agent_id', ''],
    ['/result/target/worktree_identity_digest', 'bad'],
    ['/result/revision', 0],
    ['/request/extra', true],
    ['/result/updated_by/extra', true],
    ['/result/owner/extra', true],
    ['/result/pin/extra', true],
    ['/result/target/extra', true],
    ['/extra', true],
  ] as const) {
    fetch.mockResolvedValueOnce(response(edit(pointer, value)));
    await expect(
      client.bridges.setSourceControls(grant, request, { idempotencyKey: 'same' }),
    ).rejects.toBeInstanceOf(ProtocolError);
  }
  for (const status of [201, 202, 204]) {
    fetch.mockResolvedValueOnce(
      status === 204 ? new Response(null, { status }) : response(receipt(), status),
    );
    await expect(
      client.bridges.setSourceControls(grant, request, { idempotencyKey: 'same' }),
    ).rejects.toBeInstanceOf(ProtocolError);
  }
});
it('reserves the final policy slot for explicit revocation and checks all local input before fetch', async () => {
  const { client, fetch, grant, request } = fixture();
  const bads: unknown[] = [
    { ...request, expected_bridge_revision: 0 },
    { ...request, expected_bridge_revision: Number.MAX_SAFE_INTEGER + 1 },
    { ...request, expected_grant_revision: 0 },
    { ...request, expected_grant_revision: 3 },
    { ...request, expected_policy_revision: -1 },
    { ...request, expected_policy_revision: 256 },
    { ...request, expected_policy_revision: 255 },
    { ...request, actor: 'injected' },
    { ...request, action: 'cancel' },
  ];
  for (const bad of bads)
    expect(() =>
      client.bridges.setSourceControls(grant, bad as BridgeSourceControlRequest, {
        idempotencyKey: 'same',
      }),
    ).toThrow(ValidationError);
  for (const key of ['', 'has space', 'x'.repeat(201), '雪'])
    expect(() => client.bridges.setSourceControls(grant, request, { idempotencyKey: key })).toThrow(
      ValidationError,
    );
  expect(() => client.bridges.readSourceControls('bad')).toThrow(ValidationError);
  expect(() =>
    client.bridges.setSourceControls('bad', request, { idempotencyKey: 'same' }),
  ).toThrow(ValidationError);
  expect(fetch).not.toHaveBeenCalled();
  const revoke: BridgeSourceControlRequest = {
    ...request,
    expected_policy_revision: 255,
    expected_grant_revision: 2,
    action: 'revoke_sender_controls',
  };
  const accepted = {
    ...receipt(),
    request: revoke,
    result: { ...receipt().result, revision: 256, enabled: false },
  };
  fetch.mockResolvedValueOnce(response(accepted));
  expect(
    await client.bridges.setSourceControls(grant, revoke, { idempotencyKey: 'last-revoke' }),
  ).toEqual(accepted);
});
it('aborted credentials cannot send consent', async () => {
  const { fetch, grant, request } = fixture();
  const controller = new AbortController();
  controller.abort();
  const client = createClient({
    baseUrl: 'https://consent.test',
    credential: () => 'fixture-manager',
    fetch,
  });
  await expect(
    client.bridges.setSourceControls(grant, request, {
      idempotencyKey: 'same',
      signal: controller.signal,
    }),
  ).rejects.toBeDefined();
  expect(fetch).not.toHaveBeenCalled();
});
