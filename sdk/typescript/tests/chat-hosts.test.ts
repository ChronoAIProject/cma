import { describe, expect, it, vi } from 'vitest';
import { listChatHosts, listChatHostsPage, retireChatHost, toChatHost } from '../src/chat-hosts.js';
import { Cma } from '../src/client.js';
import { resolveConfig } from '../src/config.js';
import { ProtocolError, ValidationError } from '../src/errors.js';

/** @vitest-environment node */

const HOST = {
  id: 'chho_0123456789abcdef0123456789abcdef',
  owner: 'subject-owner',
  status: 'active',
  thread_count: 3,
  runtime_version: 'runtime-v2',
  paused_at: '2026-09-01T01:00:00Z',
  revived_at: '2026-09-01T02:00:00Z',
  checkpoint_captured_at: '2026-09-01T00:45:00Z',
  settings_revision: 17,
  created_at: '2026-08-31T23:00:00Z',
} as const;

function response(body: unknown, link?: string): Response {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: {
      'content-type': 'application/json',
      ...(link === undefined ? {} : { link }),
    },
  });
}

describe('chat host wire shape', () => {
  it('decodes every documented inventory field', () => {
    expect(toChatHost(HOST)).toEqual({
      id: HOST.id,
      owner: HOST.owner,
      status: 'active',
      threadCount: 3,
      runtimeVersion: 'runtime-v2',
      pausedAt: Date.parse(HOST.paused_at),
      revivedAt: Date.parse(HOST.revived_at),
      checkpointCapturedAt: Date.parse(HOST.checkpoint_captured_at),
      settingsRevision: 17,
      createdAt: Date.parse(HOST.created_at),
    });
  });

  it('refuses malformed lifecycle, counts, and timestamps', () => {
    expect(() => toChatHost({ ...HOST, status: 'future' })).toThrow(ProtocolError);
    expect(() => toChatHost({ ...HOST, thread_count: -1 })).toThrow(ProtocolError);
    expect(() => toChatHost({ ...HOST, created_at: 'not-a-time' })).toThrow(ProtocolError);
  });
});

describe('chat host operations', () => {
  it('follows route-bound page continuations and walks all pages', async () => {
    const retired = { ...HOST, id: `${HOST.id}2`, status: 'retired' };
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(
        response([HOST], '</api/v1/admin/chat/hosts?limit=1&cursor=opaque-next>; rel="next"'),
      )
      .mockResolvedValueOnce(response([retired]))
      .mockResolvedValueOnce(
        response([HOST], '</api/v1/admin/chat/hosts?limit=50&cursor=walk-next>; rel="next"'),
      )
      .mockResolvedValueOnce(response([retired]));
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    const first = await listChatHostsPage(config, { limit: 1 });
    if (first.next === undefined) {
      throw new Error('the first page did not carry its continuation');
    }
    await expect(listChatHostsPage(config, { next: first.next })).resolves.toMatchObject({
      items: [{ status: 'retired' }],
    });
    await expect(listChatHosts(config)).resolves.toHaveLength(2);
    expect(fetch.mock.calls.map(([url]) => String(url))).toEqual([
      '/api/v1/admin/chat/hosts?limit=1',
      '/api/v1/admin/chat/hosts?limit=1&cursor=opaque-next',
      '/api/v1/admin/chat/hosts',
      '/api/v1/admin/chat/hosts?limit=50&cursor=walk-next',
    ]);
  });

  it('posts retirement, validates the result, and is exposed by Cma', async () => {
    const retired = { ...HOST, status: 'retired' };
    const fetch = vi.fn().mockImplementation(() => Promise.resolve(response(retired)));
    const config = resolveConfig({ fetch: fetch as typeof globalThis.fetch });
    await expect(retireChatHost(config, HOST.id)).resolves.toMatchObject({ status: 'retired' });
    const cma = new Cma({ fetch: fetch as typeof globalThis.fetch });
    await expect(cma.retireChatHost(HOST.id)).resolves.toMatchObject({ status: 'retired' });
    await expect(cma.listChatHosts()).rejects.toBeInstanceOf(ProtocolError);

    expect(fetch.mock.calls.slice(0, 2).map(([url, init]) => [String(url), init?.method])).toEqual([
      [`/api/v1/admin/chat/hosts/${HOST.id}/retire`, 'POST'],
      [`/api/v1/admin/chat/hosts/${HOST.id}/retire`, 'POST'],
    ]);
    await expect(retireChatHost(config, '   ')).rejects.toBeInstanceOf(ValidationError);
  });
});
