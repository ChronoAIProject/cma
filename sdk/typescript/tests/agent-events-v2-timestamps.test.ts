import { expect, it, vi } from 'vitest';
import { ProtocolError } from '../src/index.js';
import { agent, cursor, frame, harness, live, sse } from './agent-events-v2-fixtures.js';

it('negotiates live observation timestamps without requiring them from older servers', async () => {
  const t = harness();
  for (const observed_at of [undefined, '2026-10-20T16:31:00Z']) {
    t.fetch.mockResolvedValueOnce(
      sse(frame('cma.agent.live', { ...live(), ...(observed_at ? { observed_at } : {}) })),
    );
    const receive = vi.fn();
    await t.api.events(agent, {
      afterCursor: cursor(2),
      messageBindings: true,
      liveTimestamps: true,
      onEvent: receive,
    });
    expect(receive.mock.calls[0]?.[0].data.observed_at).toBe(observed_at);
    expect(new Headers(t.fetch.mock.lastCall?.[1]?.headers).get('Prefer')).toBe(
      'cma-message-bindings, cma-live-observed-at',
    );
  }
});

it.each([4, null, '', '\n'])(
  'rejects malformed live observation timestamp %j',
  async (observed_at) => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(sse(frame('cma.agent.live', { ...live(), observed_at })));
    await expect(
      t.api.events(agent, { afterCursor: cursor(2), liveTimestamps: true, onEvent: vi.fn() }),
    ).rejects.toBeInstanceOf(ProtocolError);
  },
);
