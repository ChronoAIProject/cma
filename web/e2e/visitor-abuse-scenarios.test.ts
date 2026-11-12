// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { sharedQuota, noEffects, identityBoundary } from './visitor-abuse-scenarios';
import { newCounts } from './visitor-abuse-model';
import { CallerResponseLost, type TC117Client } from './visitor-abuse-client';

function sharedDriver(mode: 'valid' | 'overshoot' | 'server-error' | 'wrong-quota' = 'valid') {
  let attempts = 0;
  const admitted: Record<string, unknown>[] = [];
  const driver = {
    fixture: {
      instances: ['https://one.example.test', 'https://two.example.test'],
      triggers: { reuseA: { publicId: 'one' }, reuseB: { publicId: 'two' } },
    },
    counts: newCounts(),
    observations: {},
    list: async () => admitted,
    launch: async (_actor: unknown, target: { publicId: string }) => {
      attempts++;
      if (mode === 'server-error' && attempts === 10) return { status: 503, body: {}, ms: 1 };
      if (attempts <= (mode === 'overshoot' ? 6 : 5)) {
        const row = {
          id: `launch-${attempts}`,
          public_id: target.publicId,
          created_at: '2026-11-13T12:00:00Z',
        };
        admitted.push(row);
        return { status: 202, body: row, ms: 1 };
      }
      return {
        status: 429,
        retryAfter: '10',
        body: {
          code: mode === 'wrong-quota' ? 'rate_limited' : 'resource_quota',
          resource: 'visitor_launches',
          limit: 5,
          reset_at: '2026-11-14T00:00:00Z',
        },
        ms: 1,
      };
    },
    settled: async () => ({
      state: 'ready',
      resolution: { sandbox: { state: 'reused', id: 'same-home' } },
    }),
  };
  return driver;
}
describe('TC-117 campaign scenario mechanics', () => {
  it('runs all fifty concurrent keys across both origins and Triggers and reconciles five receipts', async () => {
    const driver = sharedDriver();
    await sharedQuota(driver as unknown as TC117Client);
    expect(driver.counts.overquotaAttempts).toBe(45);
    expect(driver.counts.overquotaRejects).toBe(45);
  });
  it('accepts a home seeded on an earlier UTC day without treating historical receipts as new usage', async () => {
    const driver = sharedDriver();
    const list = driver.list;
    driver.list = async () => [
      { id: 'historical-seed', created_at: '2020-01-01T00:00:00Z' },
      ...(await list()),
    ];
    await sharedQuota(driver as unknown as TC117Client);
    expect(driver.counts.overquotaRejects).toBe(45);
  });
  it.each(['overshoot', 'server-error', 'wrong-quota'] as const)(
    'fails the actual campaign on %s',
    async (mode) => {
      await expect(sharedQuota(sharedDriver(mode) as unknown as TC117Client)).rejects.toThrow();
    },
  );
  it('requires zero public side effects, recording observed increments even on failure', () => {
    const counts = newCounts();
    const snapshot = {
      fleetLive: 1,
      launches: { a: ['one'] },
      sandboxes: ['home'],
      totals: { trigger: { launches: 1, ready: 1 } },
      liveByVisitor: { a: 1 },
    };
    expect(() => noEffects(snapshot, snapshot, counts)).not.toThrow();
    expect(counts.anonymousSideEffects).toBe(0);
    expect(() =>
      noEffects(snapshot, { ...snapshot, launches: { a: ['one', 'two'] } }, counts),
    ).toThrow();
    expect(counts.anonymousSideEffects).toBe(1);
  });
  it('refuses an unexpectedly powerful visitor before mutation', async () => {
    await expect(
      identityBoundary(
        {
          authenticated: true,
          subject: 'a',
          visitor: true,
          capabilities: [
            'launch_triggers',
            'use_own_sandboxes',
            'delete_own_sandbox',
            'create_sandbox',
          ],
        },
        'a',
        true,
      ),
    ).rejects.toThrow();
    await expect(
      identityBoundary(
        {
          authenticated: true,
          subject: 'a',
          visitor: true,
          capabilities: ['launch_triggers', 'use_own_sandboxes', 'delete_own_sandbox'],
        },
        'a',
        true,
      ),
    ).resolves.toBeUndefined();
  });
});

import { vi } from 'vitest';
import { utcScenario } from './visitor-abuse-scenarios';

it.each([false, true])(
  'executes UTC operations only inside both real midnight windows (late replay: %s)',
  async (late) => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-11-13T23:59:40Z'));
    const durable = new Map<
      string,
      { id: string; created_at: string; public_id: string; state: string }
    >();
    const keys = new Map<string, { payload: string; row: Record<string, unknown> }>();
    let sequence = 0;
    let responseLosses = 0;
    const driver = {
      fixture: {
        instances: ['https://one.example.test', 'https://two.example.test'],
        triggers: {
          reuseA: { publicId: 'one', placementId: 'one-place' },
          reuseB: { publicId: 'two', placementId: 'two-place' },
        },
      },
      counts: newCounts(),
      observations: {},
      launch: async (
        _actor: string,
        target: { publicId: string; placementId: string },
        options: { key?: string; data?: unknown } = {},
      ) => {
        const scope = `${target.publicId}:${options.key ?? `new-${sequence++}`}`;
        const payload = JSON.stringify(
          options.data ?? { placement_id: target.placementId, start_if_ready: true },
        );
        const prior = keys.get(scope);
        if (prior)
          return prior.payload === payload
            ? { status: 202, body: prior.row, ms: 1 }
            : { status: 409, body: { code: 'idempotency_key_reused' }, ms: 1 };
        const today = new Date().toISOString().slice(0, 10);
        if ([...durable.values()].filter((row) => row.created_at.startsWith(today)).length >= 5)
          return {
            status: 429,
            body: {
              code: 'resource_quota',
              resource: 'visitor_launches',
              limit: 5,
              reset_at: new Date((Math.floor(Date.now() / 86400000) + 1) * 86400000).toISOString(),
            },
            retryAfter: '10',
            ms: 1,
          };
        const row = {
          id: `launch-${sequence++}`,
          public_id: target.publicId,
          created_at: new Date().toISOString(),
          state: 'draft',
        };
        durable.set(row.id, row);
        keys.set(scope, { payload, row });
        return { status: 202, body: row, ms: 1 };
      },
      async launchAndLoseResponse(
        actor: string,
        target: { publicId: string; placementId: string },
        options: { key?: string; data?: unknown } = {},
      ) {
        await this.launch(actor, target, options);
        responseLosses++;
        throw new CallerResponseLost();
      },
      list: async () => {
        const boundary = Date.parse('2026-11-14T00:00:00Z');
        if (late && Date.now() >= boundary && Date.now() < boundary + 31000)
          vi.setSystemTime(new Date(boundary + 31000));
        return [...durable.values()];
      },
      control: async (_actor: string, _kind: string, id: string) => {
        durable.get(id)!.state = 'cancelled';
        return { status: 200, ms: 1 };
      },
    };
    try {
      const pending = utcScenario(driver as unknown as TC117Client).then(
        () => 'passed',
        (error) => String(error),
      );
      await vi.runAllTimersAsync();
      const outcome = await pending;
      if (late) {
        expect(outcome).toContain('post-midnight');
        return;
      }
      expect(outcome).toBe('passed');
      expect(responseLosses).toBe(1);
      expect(durable.size).toBe(10);
      expect(driver.counts.idempotentChargeDuplicates).toBe(0);
      expect(driver.counts.idempotencyChecks).toBe(3);
      expect([...durable.values()].filter((row) => row.state === 'cancelled')).toHaveLength(1);
    } finally {
      vi.useRealTimers();
    }
  },
);

import { boundaryScenario } from './visitor-abuse-scenarios';
it('reconciles only a proved pre-allocation terminal refusal and retains all other cleanup ownership', async () => {
  let launches = 0;
  let homes = 0;
  const resources: { kind: string; id: string; subject: string }[] = [];
  const records: unknown[] = [];
  const driver = {
    fixture: {
      triggers: { alwaysNew: { id: 'trigger', publicId: 'public', placementId: 'place' } },
      accounts: { boundary: { subject: 'visitor' } },
      sandboxProfile: { id: 'profile', revision: 1 },
    },
    counts: newCounts(),
    observations: {},
    journal: { resources, record: (row: unknown) => records.push(row) },
    send: async (actor: string) => ({
      status: actor === 'anonymous' ? 401 : 403,
      body: { code: 'refused' },
      ms: 1,
    }),
    snapshot: async () => ({
      launches: { boundary: Array.from({ length: launches }, (_, i) => `launch-${i + 1}`) },
      sandboxes: Array.from({ length: homes }, (_, i) => `home-${i + 1}`),
      liveByVisitor: { boundary: homes },
      totals: { trigger: { launches, ready: homes } },
    }),
    launch: async () => {
      launches++;
      resources.push({ kind: 'launches', id: `launch-${launches}`, subject: 'visitor' });
      return {
        status: 202,
        body: { id: `launch-${launches}`, created_at: '2026-11-13T12:00:00Z' },
        ms: 1,
      };
    },
    settled: async () => {
      if (launches <= 2) {
        homes++;
        return {
          id: `launch-${launches}`,
          state: 'ready',
          resolution: { sandbox: { state: 'created', id: `home-${homes}` } },
        };
      }
      return {
        id: 'launch-3',
        state: 'failed_terminal',
        failure: { code: 'sandbox_limit_reached' },
        resolution: {
          sandbox: { state: 'pending' },
          workspace: { state: 'pending' },
          agent: { state: 'pending' },
        },
      };
    },
  };
  await boundaryScenario(driver as unknown as TC117Client);
  expect(resources.map((row) => row.id)).toEqual(['launch-1', 'launch-2']);
  expect(records).toContainEqual(
    expect.objectContaining({ action: 'terminal_refusal_reconciled', id: 'launch-3' }),
  );
});
