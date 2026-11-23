// @vitest-environment node
import { afterEach, describe, expect, it, vi } from 'vitest';
import { runLoadWave } from './visitor-abuse-load';
import { newCounts } from './visitor-abuse-model';
import type { TC117Client } from './visitor-abuse-client';
import type { APIRequestContext } from '@playwright/test';

afterEach(() => vi.useRealTimers());
describe('TC-117 bounded anonymous load mechanics', () => {
  it('counts offered, admitted and typed refusals across both serving and egress paths', async () => {
    vi.useFakeTimers();
    const calls: unknown[] = [];
    const counts = newCounts();
    const driver = {
      counts,
      fixture: { instances: ['one', 'two'], triggers: { reuseA: { publicId: 'target' } } },
      send: async (_actor: string, path: string, options: unknown) => {
        calls.push({ path, options });
        return calls.length % 2
          ? { status: 200, body: {}, ms: 1 }
          : { status: 429, body: { code: 'rate_limited' }, retryAfter: '1', ms: 1 };
      },
    };
    const proxies = [{ name: 'first' }, { name: 'second' }] as unknown as APIRequestContext[];
    const pending = runLoadWave(
      driver as unknown as TC117Client,
      proxies,
      4,
      1,
      async () => {},
    ).then(
      () => 'passed',
      () => 'failed',
    );
    await vi.runAllTimersAsync();
    expect(await pending).toBe('passed');
    expect(counts).toMatchObject({ offered: 4, admitted: 2, refused: 2 });
    expect(calls).toHaveLength(4);
    expect(
      new Set(calls.map((call) => (call as { options: { origin: string } }).options.origin)).size,
    ).toBe(2);
  });
  it('records an unexpected failure instead of counting it as an admitted request', async () => {
    vi.useFakeTimers();
    const counts = newCounts();
    const driver = {
      counts,
      fixture: { instances: ['one', 'two'], triggers: { reuseA: { publicId: 'target' } } },
      send: async () => ({ status: 503, body: {}, ms: 1 }),
    };
    const pending = runLoadWave(
      driver as unknown as TC117Client,
      [{}] as APIRequestContext[],
      1,
      1,
      async () => {},
    ).then(
      () => 'passed',
      () => 'failed',
    );
    await vi.runAllTimersAsync();
    expect(await pending).toBe('failed');
    expect(counts).toMatchObject({ offered: 1, admitted: 0, refused: 0 });
  });
});

it('keeps the offered load schedule independent of slow control-group responses', async () => {
  vi.useFakeTimers();
  const counts = newCounts();
  const driver = {
    counts,
    fixture: { instances: ['one', 'two'], triggers: { reuseA: { publicId: 'target' } } },
    send: async () => ({ status: 200, body: {}, ms: 1 }),
  };
  const pending = runLoadWave(
    driver as unknown as TC117Client,
    [{}] as APIRequestContext[],
    4,
    2,
    async () => {
      await new Promise((resolve) => setTimeout(resolve, 5000));
    },
  ).then(
    () => 'passed',
    () => 'failed',
  );
  await vi.advanceTimersByTimeAsync(2000);
  expect(counts.offered).toBe(8);
  await vi.runAllTimersAsync();
  expect(await pending).toBe('passed');
});

it.each([0, 5000])(
  'fails when only legacy network buckets remain, including slow cohort tails (%s ms)',
  async (tailMs) => {
    vi.useFakeTimers();
    const counts = newCounts();
    const started = Date.now();
    const buckets = Array.from({ length: 6 }, () => ({ tokens: 6, at: started }));
    const proxies = buckets.map((_, index) => ({ index })) as unknown as APIRequestContext[];
    const driver = {
      counts,
      observations: {},
      fixture: {
        instances: ['one', 'two'],
        triggers: { reuseA: { publicId: 'target' } },
        load: { perPodPerMinute: 12, maxServingPods: 2 },
      },
      send: async (_actor: string, _path: string, options: { api: { index: number } }) => {
        const bucket = buckets[options.api.index]!;
        bucket.tokens = Math.min(6, bucket.tokens + ((Date.now() - bucket.at) * 6) / 60000);
        bucket.at = Date.now();
        if (bucket.tokens >= 1) {
          bucket.tokens--;
          return { status: 200, body: {}, ms: 1 };
        }
        return { status: 429, body: { code: 'rate_limited' }, retryAfter: '1', ms: 1 };
      },
    };
    const pending = runLoadWave(driver as unknown as TC117Client, proxies, 2, 30, async () => {
      await new Promise((resolve) => setTimeout(resolve, tailMs));
    }).then(
      () => 'passed',
      (error) => String(error),
    );
    await vi.runAllTimersAsync();
    expect(await pending).toContain('frozen fleet');
    expect(counts.refused).toBeGreaterThan(0);
    expect(counts.admitted).toBeGreaterThan(36);
  },
);
