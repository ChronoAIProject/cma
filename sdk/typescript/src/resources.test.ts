import { describe, expect, it, vi } from 'vitest';
import { resolveConfig } from './config';
import { readResources, toResourceStatus } from './resources';

/** @vitest-environment node */

const EMPTY_READING = {
  cpu_millis_used: null,
  cpu_millis_limit: null,
  memory_bytes_used: null,
  memory_bytes_limit: null,
  memory_bytes_peak: null,
  stall_permille: null,
  ceiling_hits: null,
  allocation_failures: null,
  kills: null,
  received_bytes: null,
  sent_bytes: null,
  received_bytes_per_second: null,
  sent_bytes_per_second: null,
  interval_ms: null,
} as const;

function answering(body: unknown) {
  const fetch = vi.fn(() =>
    Promise.resolve({
      ok: true,
      status: 200,
      json: () => Promise.resolve(body),
    } as Response),
  );
  return { fetch: fetch as unknown as typeof globalThis.fetch, calls: fetch };
}

describe('the resource point read', () => {
  it('reads the public route and translates all fourteen snake-case fields', async () => {
    const { fetch, calls } = answering({
      reading: {
        ...EMPTY_READING,
        cpu_millis_used: 340,
        cpu_millis_limit: 1000,
        memory_bytes_used: 892_211_200,
        ceiling_hits: 0,
      },
      observed_at: '2026-08-23T04:12:00Z',
      reason: null,
    });

    const status = await readResources(resolveConfig({ fetch }), 'sesn/one');

    expect(calls).toHaveBeenCalledWith(
      '/api/v1/sessions/sesn%2Fone/resources',
      expect.objectContaining({ method: 'GET' }),
    );
    expect(Object.keys(status.reading ?? {})).toHaveLength(14);
    expect(status).toMatchObject({
      observedAt: '2026-08-23T04:12:00Z',
      reading: {
        cpuMillisUsed: 340,
        cpuMillisLimit: 1000,
        memoryBytesUsed: 892_211_200,
        memoryBytesLimit: undefined,
        ceilingHits: 0,
      },
    });
    expect(status.reason).toBeUndefined();
  });

  it('keeps waiting and permanently unreported as different answers', () => {
    expect(
      toResourceStatus({ reading: null, observed_at: null, reason: 'no_reading_yet' }),
    ).toEqual({ reason: 'no_reading_yet' });
    expect(
      toResourceStatus({
        reading: null,
        observed_at: null,
        reason: 'runtime_does_not_report',
      }),
    ).toEqual({ reason: 'runtime_does_not_report' });
  });

  it('refuses an omitted figure instead of quietly reading it as absent', () => {
    const missing = Object.fromEntries(
      Object.entries(EMPTY_READING).filter(([field]) => field !== 'interval_ms'),
    );
    expect(() =>
      toResourceStatus({
        reading: missing,
        observed_at: '2026-08-23T04:12:00Z',
        reason: null,
      }),
    ).toThrow('omitted interval_ms');
  });
});
