import { describe, expect, it, vi } from 'vitest';
import { Cmaeg } from '../src/index.js';
import { OutboundCommitUncertainError } from '../src/cmaeg/index.js';
const body = { source_id: 'relay_telegram', route_id: 'saved-route', message: { text: 'hello' } };
const receipt = (status = 'prepared', delivery: unknown = null) => ({
  delivery_id: 'delivery-one',
  source_id: body.source_id,
  route_id: body.route_id,
  status,
  expires_at: 1900000000,
  retained_until: 1900500000,
  rendering: { outcome: 'rendered', blocks: [] },
  delivery,
});
const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status, headers: { 'content-type': 'application/json' } });
describe('owner-initiated messages', () => {
  it('prepares and commits the exact typed body once', async () => {
    const fetch = vi
      .fn<typeof globalThis.fetch>()
      .mockResolvedValueOnce(json(receipt(), 201))
      .mockResolvedValueOnce(
        json(receipt('committed', { outcome: 'confirmed', platform_message_id: 'native' })),
      );
    const api = new Cmaeg({
      baseUrl: 'https://gateway.example',
      credential: () => 'fixture',
      fetch,
    });
    await api.sendOutboundMessage('channel/one', body, { idempotencyKey: 'logical-one' });
    expect(fetch).toHaveBeenCalledTimes(2);
    expect(String(fetch.mock.calls[0]?.[0])).toContain(
      '/v1/channels/channel%2Fone/outbound-messages',
    );
    expect(fetch.mock.calls[0]?.[1]?.body).toBe(JSON.stringify(body));
    expect(fetch.mock.calls[1]?.[1]?.body).toBe(JSON.stringify(body.message));
    expect(String(fetch.mock.calls[1]?.[0])).toContain('/delivery-one/send');
  });
  it('exposes ambiguous commit reference and does not retry', async () => {
    const fetch = vi
      .fn<typeof globalThis.fetch>()
      .mockResolvedValueOnce(json(receipt(), 201))
      .mockRejectedValueOnce(new TypeError('lost'));
    const api = new Cmaeg({
      baseUrl: 'https://gateway.example',
      credential: () => 'fixture',
      fetch,
    });
    const error = await api
      .sendOutboundMessage('channel', body, { idempotencyKey: 'logical-one' })
      .catch((error: unknown) => error);
    expect(error).toBeInstanceOf(OutboundCommitUncertainError);
    expect(error).toMatchObject({ deliveryId: 'delivery-one', isRetryable: false });
    expect(fetch).toHaveBeenCalledTimes(2);
  });
  it('never commits a replayed unknown or cancelled preparation', async () => {
    for (const saved of [
      receipt('cancelled'),
      receipt('committed', { outcome: 'unknown', reason: 'lost' }),
    ]) {
      const fetch = vi.fn<typeof globalThis.fetch>().mockResolvedValueOnce(json(saved, 201));
      const api = new Cmaeg({
        baseUrl: 'https://gateway.example',
        credential: () => 'fixture',
        fetch,
      });
      expect(
        await api.sendOutboundMessage('channel', body, { idempotencyKey: 'logical-one' }),
      ).toEqual(saved);
      expect(fetch).toHaveBeenCalledTimes(1);
    }
  });
  it('reads and cancels a receipt without resending and rejects invalid keys locally', async () => {
    const fetch = vi.fn<typeof globalThis.fetch>().mockImplementation(async () => json(receipt()));
    const api = new Cmaeg({
      baseUrl: 'https://gateway.example',
      credential: () => 'fixture',
      fetch,
    });
    await api.getOutboundMessage('channel', 'delivery-one');
    await api.cancelOutboundMessage('channel', 'delivery-one');
    expect(fetch.mock.calls.map((call) => call[1]?.method)).toEqual(['GET', 'DELETE']);
    expect(() => api.prepareOutboundMessage('channel', body, { idempotencyKey: '' })).toThrow();
    expect(fetch).toHaveBeenCalledTimes(2);
  });
});
