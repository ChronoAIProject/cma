import { expect, it, vi } from 'vitest';
import { createClient, ProtocolError } from '../src/index.js';

const observation = {
  agent_id: 'agent-a',
  runtime_incarnation: 'incarnation',
  context_id: 'context',
  state: 'live',
  location: 'Local page',
  observers: 1,
  captures: 1,
  bytes: 3,
  min_interval_ms: 1000,
  fresh_for_ms: 3000,
  max_frame_bytes: 262144,
  max_observers: 16,
  frame: {
    sequence: 1,
    captured_at: '2026-11-03T00:00:00Z',
    media_type: 'image/jpeg',
    width: 1280,
    height: 720,
    data: '/9j/',
  },
};

it('reads bounded frames from the authorized Agent and rejects another runtime or oversized frame', async () => {
  const fetch = vi
    .fn()
    .mockResolvedValueOnce(Response.json(observation))
    .mockResolvedValueOnce(Response.json({ ...observation, runtime_incarnation: 'old' }))
    .mockResolvedValueOnce(
      Response.json({ ...observation, frame: { ...observation.frame, data: 'a'.repeat(350000) } }),
    );
  const client = createClient({ baseUrl: 'https://cma.example', fetch });
  const scope = { runtime_incarnation: 'incarnation', observer: 'observer', context_id: 'context' };
  expect(await client.agentBrowser.read('agent-a', scope)).toEqual(observation);
  expect(String(fetch.mock.calls[0]?.[0])).toContain('/agents/agent-a/browser?');
  await expect(client.agentBrowser.read('agent-a', scope)).rejects.toThrow(ProtocolError);
  await expect(client.agentBrowser.read('agent-a', scope)).rejects.toThrow(ProtocolError);
});

it.each([undefined, 'about:blank', 'http://localhost:5173', 'https://example.com'])(
  'accepts an optional sanitized browser address: %s',
  async (address) => {
    const fetch = vi.fn().mockResolvedValue(Response.json({ ...observation, address }));
    const client = createClient({ baseUrl: 'https://cma.example', fetch });
    expect(
      (await client.agentBrowser.read('agent-a', { runtime_incarnation: 'incarnation' })).address,
    ).toBe(address);
  },
);

it.each([
  'https://example.com/private',
  'https://example.com?private',
  'https://example.com#private',
  'https://user:pass@example.com',
  'file:///home/private',
  'javascript:alert(1)',
])('rejects a browser address beyond the origin: %s', async (address) => {
  const fetch = vi.fn().mockResolvedValue(Response.json({ ...observation, address }));
  const client = createClient({ baseUrl: 'https://cma.example', fetch });
  await expect(
    client.agentBrowser.read('agent-a', { runtime_incarnation: 'incarnation' }),
  ).rejects.toThrow(ProtocolError);
});
