import { describe, expect, it, vi } from 'vitest';
import { Cmaeg, ValidationError } from '../src/index.js';
import { CredentialsRequestSchema, SourceRequestSchema } from '../src/cmaeg/types.js';

const json = (value: unknown, status = 200) =>
  new Response(JSON.stringify(value), { status, headers: { 'content-type': 'application/json' } });

describe('gateway connection client', () => {
  it('keeps creator, originating agent and per-operation organization authority separate', async () => {
    const fetch = vi.fn<typeof globalThis.fetch>();
    const client = new Cmaeg({
      baseUrl: 'https://gateway.example',
      credential: () => 'creator',
      fetch,
    });
    const view = {
      connection_ref: 'opaque/+?',
      status: 'draft',
      label: null,
      url: null,
      setup_url: null,
    };
    fetch.mockResolvedValueOnce(json({ platforms: [] }));
    expect(await client.getChannelCapabilities()).toEqual({ platforms: [] });
    const create = {
      record_id: 'reserved-uuid',
      event_ref: 'sealed-event',
      suggested_name: 'Helper',
    };
    fetch.mockResolvedValueOnce(json(view, 201));
    expect(
      await client.createChannelConnection(create, { owner: 'org-id', agentKey: 'origin-agent' }),
    ).toEqual(view);
    const creation = fetch.mock.calls.at(-1);
    expect(new Headers(creation?.[1]?.headers).get('authorization')).toBe('Bearer creator');
    expect(new Headers(creation?.[1]?.headers).get('x-cmaeg-agent-authorization')).toBe(
      'Bearer origin-agent',
    );
    expect(new Headers(creation?.[1]?.headers).get('x-cmaeg-owner')).toBe('org-id');
    expect(creation?.[1]?.body).toBe(JSON.stringify(create));
    expect(creation?.[1]?.method).toBe('POST');
    fetch.mockResolvedValueOnce(json({ items: [], next_after: 'cursor/+?' }));
    expect(await client.listChannelConnections({ owner: 'org-id' })).toEqual({
      items: [],
      next_after: 'cursor/+?',
    });
    fetch.mockResolvedValueOnce(json({ items: [view], next_after: null }));
    await client.listChannelConnections({ after: 'cursor/+?' });
    const listing = fetch.mock.calls.at(-1);
    expect(listing?.[0]).toBe(
      'https://gateway.example/v1/channel-connections?after=cursor%2F%2B%3F',
    );
    expect(new Headers(listing?.[1]?.headers).has('x-cmaeg-owner')).toBe(false);
    expect(new Headers(listing?.[1]?.headers).has('x-cmaeg-agent-authorization')).toBe(false);
    fetch.mockResolvedValueOnce(json(view));
    await client.getChannelConnection(view.connection_ref);
    expect(fetch.mock.calls.at(-1)?.[0]).toBe(
      'https://gateway.example/v1/channel-connections/opaque%2F%2B%3F',
    );
    fetch.mockResolvedValueOnce(new Response(null, { status: 204 }));
    await client.cancelChannelConnection(view.connection_ref, { owner: 'org-id' });
    expect(fetch.mock.calls.at(-1)?.[1]?.method).toBe('DELETE');
    expect(new Headers(fetch.mock.calls.at(-1)?.[1]?.headers).get('x-cmaeg-owner')).toBe('org-id');
    expect(fetch).toHaveBeenCalledTimes(6);
  });

  it('rejects malformed authority locally and retains native Telegram input fields', () => {
    const fetch = vi.fn<typeof globalThis.fetch>();
    const client = new Cmaeg({
      baseUrl: 'https://gateway.example',
      credential: () => 'creator',
      fetch,
    });
    const body = { record_id: 'reserved', event_ref: 'event' };
    expect(() => client.createChannelConnection(body, { agentKey: 'bad\ncredential' })).toThrow(
      ValidationError,
    );
    expect(() => client.listChannelConnections({ owner: 'bad owner' })).toThrow(ValidationError);
    expect(fetch).not.toHaveBeenCalled();
    const source = {
      type: 'telegram',
      bot_id: '1234',
      bot_username: 'helper_bot',
      admission: { type: 'all' },
    };
    expect(SourceRequestSchema(source)).toEqual(source);
    const credentials = {
      agent_key: 'fixture',
      telegram_bot_token: 'bot-fixture',
      telegram_webhook_secret: 'webhook-fixture',
    };
    expect(CredentialsRequestSchema(credentials)).toEqual(credentials);
  });
});
