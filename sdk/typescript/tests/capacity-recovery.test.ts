import { describe, expect, it } from 'vitest';
import { ApiError, ProtocolError } from '../src/index.js';
import { transport } from './resource-fixtures.js';

const original = {
  agent_id: 'agt_recovery',
  creation_key: 'creation',
  creation_fingerprint: 'a'.repeat(64),
  failure_seq: 10001,
  requested_at: '2026-11-11T00:00:00Z',
};

for (const method of ['admit', 'resume'] as const) {
  describe(method, () => {
    it('preserves a known abort with one request and no implicit retry', async () => {
      const f = transport();
      f.answer({ outcome: 'known_abort' }, 409);
      expect(await f.client.agents.capacityRecovery(original.agent_id)[method](original)).toEqual({
        outcome: 'known_abort',
      });
      expect(f.calls).toHaveLength(1);
      expect(f.calls[0]!.url).toContain(`/capacity-recovery/${method}`);
      expect(JSON.parse(f.calls[0]!.init.body as string)).toEqual(original);
    });

    it('keeps a conflict problem as an API error with its metadata', async () => {
      const f = transport();
      f.fetch.mockImplementationOnce(() =>
        Promise.resolve(
          new Response(
            JSON.stringify({
              status: 409,
              code: 'capacity_recovery_conflict',
              detail: 'Recovery evidence changed.',
            }),
            {
              status: 409,
              headers: {
                'content-type': 'application/problem+json',
                'x-request-id': 'recovery-proof',
                'retry-after': '2',
              },
            },
          ),
        ),
      );
      const result = f.client.agents.capacityRecovery(original.agent_id)[method](original);
      await expect(result).rejects.toBeInstanceOf(ApiError);
      await expect(result).rejects.toMatchObject({
        status: 409,
        code: 'capacity_recovery_conflict',
        requestId: 'recovery-proof',
        retryAfterMs: 2000,
      });
      expect(f.fetch).toHaveBeenCalledTimes(1);
    });

    it.each([
      [409, null],
      [409, {}],
      [409, { outcome: 'unknown' }],
      [409, { outcome: 'known_abort', unexpected: true }],
      [200, { outcome: 'known_abort' }],
      [202, { outcome: 'known_abort' }],
      [200, { outcome: 'unknown' }],
    ])('refuses malformed or mismatched outcome %s %j', async (status, body) => {
      const f = transport();
      f.answer(body, status as number);
      await expect(
        f.client.agents.capacityRecovery(original.agent_id)[method](original),
      ).rejects.toBeInstanceOf(ProtocolError);
      expect(f.calls).toHaveLength(1);
    });
  });
}
