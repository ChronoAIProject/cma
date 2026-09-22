import { expect, it, vi } from 'vitest';
import { createClient, ProtocolError } from '../src/index.js';

const bridge = 'brg_pending';
const allocation = 'bral_' + 'a'.repeat(64);
const value = () => ({
  status: 'available',
  bridge_id: bridge,
  partition_id: allocation,
  generation: 1,
  agent_id: 'agt_pending',
  through_sequence: 42,
  messages: [
    {
      command_id: 'cmd_one',
      response_id: 'resp_one',
      agent_order: 1,
      admitted_at: '2026-09-18T03:04:20Z',
      platform: 'telegram',
      text: 'Incoming message',
      text_truncated: false,
    },
  ],
  pending_truncated: false,
  source_unavailable: false,
  observed_at_ms: 1789700660000,
});
const response = (body: unknown) =>
  new Response(JSON.stringify(body), { headers: { 'Content-Type': 'application/json' } });

it('reads bounded previews using only the exact partition GET', async () => {
  const fetch = vi.fn().mockResolvedValue(response(value()));
  const sdk = createClient({ baseUrl: 'https://fixture.test', credential: () => 'fixture', fetch });
  expect(await sdk.bridges.pendingMessages(bridge, allocation)).toEqual(value());
  expect(new URL(fetch.mock.calls[0]![0]).pathname).toBe(
    `/api/v1/bridges/${bridge}/partitions/${allocation}/pending-messages`,
  );
  expect(fetch.mock.calls[0]![1]).toMatchObject({ method: 'GET' });
  expect(fetch.mock.calls[0]![1].body).toBeUndefined();
});

it('rejects leaked fields, excessive text, repeated identities and changed target scope', async () => {
  const fetch = vi.fn();
  const sdk = createClient({ baseUrl: 'https://fixture.test', credential: () => 'fixture', fetch });
  for (const body of [
    { ...value(), bridge_id: 'brg_other' },
    { ...value(), through_sequence: -1 },
    { ...value(), messages: [{ ...value().messages[0], reply_token: 'private' }] },
    { ...value(), messages: [{ ...value().messages[0], text: '😀'.repeat(1025) }] },
    { ...value(), messages: [value().messages[0], value().messages[0]] },
    { status: 'redacted', messages: value().messages },
  ]) {
    fetch.mockResolvedValueOnce(response(body));
    await expect(sdk.bridges.pendingMessages(bridge, allocation)).rejects.toBeInstanceOf(
      ProtocolError,
    );
  }
  fetch.mockResolvedValueOnce(response({ status: 'redacted' }));
  expect(await sdk.bridges.pendingMessages(bridge, allocation)).toEqual({ status: 'redacted' });
});
