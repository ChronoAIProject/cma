import { expect, it, vi } from 'vitest';
import {
  createClient,
  ProtocolError,
  TransportError,
  ValidationError,
  type CreateBot,
} from '../src/index.js';

const body: CreateBot = {
  name: 'Test bot',
  nyxid_bot_id: 'external-bot',
  profile: { id: 'profile', revision: 1 },
  sandbox_profile: { id: 'sbp_research', revision: 2 },
  workspace_profile: { id: 'wkp_research', revision: 3 },
  admission: { type: 'scoped', senders: { type: 'open' }, chats: { type: 'open' }, groups: 'deny' },
  reply: { auto_final: true, ack_after_ms: 0 },
};
const bot = {
  bot_id: 'bot_one',
  owner: 'owner',
  name: 'Test bot',
  nyxid_bot_id: 'external-bot',
  profile: body.profile,
  sandbox_profile: body.sandbox_profile,
  workspace_profile: body.workspace_profile,
  provider: 'cma_codex',
  state: 'creating',
  failure: null,
  operation: { operation: 'create', step: 'admitted' },
  channel: null,
  credentials_expire_at: null,
  created_at: 'now',
  updated_at: 'now',
  last_test: null,
};
function response(value: unknown, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

it('preserves the selected policies and creation key without retrying an unknown result', async () => {
  const fetch = vi
    .fn()
    .mockRejectedValueOnce(new TypeError('lost reply'))
    .mockResolvedValueOnce(response({ ...bot, future_field: true }, 201));
  const client = createClient({ baseUrl: 'https://cma.test', credential: () => 'fixture', fetch });
  await expect(client.bots.create(body, { idempotencyKey: 'original-key' })).rejects.toBeInstanceOf(
    TransportError,
  );
  expect(fetch).toHaveBeenCalledTimes(1);
  expect(await client.bots.create(body, { idempotencyKey: 'original-key' })).toEqual(bot);
  for (const call of fetch.mock.calls) {
    expect(new URL(call[0]).pathname).toBe('/api/v1/bots');
    expect(call[1].method).toBe('POST');
    expect(new Headers(call[1].headers).get('idempotency-key')).toBe('original-key');
    expect(JSON.parse(call[1].body)).toEqual(body);
  }
});
it('exposes the complete lifecycle and keeps opaque channel values unchanged', async () => {
  const fetch = vi.fn();
  const client = createClient({ baseUrl: 'https://cma.test/prefix', fetch });
  const active = {
    ...bot,
    state: 'active',
    operation: null,
    channel: {
      channel_id: 'opaque.reference/with?bytes',
      record_id: 'record',
      version: 2,
      binding_id: 'binding',
      level: 'l2',
      callback_url: 'https://gateway.test/callback',
      event_url: 'https://gateway.test/event',
    },
  };
  fetch.mockResolvedValueOnce(response(active));
  expect((await client.bots.get('bot_one')).channel).toEqual(active.channel);
  fetch.mockResolvedValueOnce(response({ items: [active], next_after: 'bot_one' }));
  expect((await client.bots.list({ after: 'before', limit: 1 })).next_after).toBe('bot_one');
  fetch.mockResolvedValueOnce(
    response([{ id: 'external-bot', name: 'Test bot', platform: 'telegram' }]),
  );
  expect(await client.bots.available()).toHaveLength(1);
  const result = { accepted_after_ms: 15, first_text: 'Ready', level: 'l2', tested_at: 'now' };
  fetch.mockResolvedValueOnce(response(result));
  expect(await client.bots.test('bot_one')).toEqual(result);
  for (const [action, suffix, method] of [
    ['disable', '/disable', 'POST'],
    ['enable', '/enable', 'POST'],
    ['rotateKeys', '/rotate-keys', 'POST'],
    ['delete', '', 'DELETE'],
  ] as const) {
    fetch.mockResolvedValueOnce(response(active, 202));
    await client.bots[action]('bot_one');
    const call = fetch.mock.calls.at(-1)!;
    expect(new URL(call[0]).pathname).toBe(`/prefix/api/v1/bots/bot_one${suffix}`);
    expect(call[1].method).toBe(method);
  }
  fetch.mockResolvedValueOnce(response({ ...bot, state: 'future-success' }));
  await expect(client.bots.get('bot_one')).rejects.toBeInstanceOf(ProtocolError);
});

it('creates a consumer from a gateway connection without a legacy bot identifier', async () => {
  const connection_ref = 'opaque-gateway-connection';
  const fetch = vi.fn().mockResolvedValueOnce(response([{ connection_ref, label: 'Research' }]));
  const client = createClient({ baseUrl: 'https://cma.test', fetch });
  expect(await client.bots.connections()).toEqual([{ connection_ref, label: 'Research' }]);
  expect(new URL(fetch.mock.calls[0]![0]).pathname).toBe('/api/v1/bots/connections');
  const request = { ...body, nyxid_bot_id: undefined, connection_ref };
  fetch.mockResolvedValueOnce(response({ ...bot, nyxid_bot_id: '', connection_ref }, 201));
  expect(
    (await client.bots.create(request, { idempotencyKey: 'connection-request' })).connection_ref,
  ).toBe(connection_ref);
  expect(JSON.parse(fetch.mock.calls[1]![1].body)).toEqual({
    ...body,
    nyxid_bot_id: undefined,
    connection_ref,
  });
  for (const invalid of [
    { ...body, connection_ref },
    { ...body, nyxid_bot_id: undefined },
  ]) {
    expect(() => client.bots.create(invalid, { idempotencyKey: 'invalid' })).toThrow(
      ValidationError,
    );
  }
  expect(fetch).toHaveBeenCalledTimes(2);
});
