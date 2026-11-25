// @vitest-environment node
import { describe, expect, it } from 'vitest';
import {
  assertQuotaRefusal,
  assertControls,
  requireInstances,
  utcBoundary,
  metrics,
  newCounts,
  admissionDays,
  assertAnonymousBudget,
  assertLoadSeparation,
} from './visitor-abuse-model';

describe('TC-117 acceptance contracts', () => {
  it('blocks missing dual serving and UTC prerequisites without a skip', () => {
    expect(() => requireInstances([])).toThrow(/^BLOCKED:/);
    expect(() => utcBoundary(Date.parse('2026-11-13T12:00:00Z'))).toThrow(/^BLOCKED:/);
    expect(() => utcBoundary(Date.parse('2026-11-14T00:00:01Z'))).toThrow(/^BLOCKED:/);
    expect(utcBoundary(Date.parse('2026-11-13T23:59:30Z'))).toBe(
      Date.parse('2026-11-14T00:00:00Z'),
    );
    expect(requireInstances(['https://one.example.test', 'https://two.example.test'])).toHaveLength(
      2,
    );
  });
  it('requires the entire typed UTC daily refusal contract', () => {
    const result = {
      status: 429,
      body: {
        code: 'resource_quota',
        resource: 'visitor_launches',
        limit: 5,
        reset_at: '2026-11-14T00:00:00Z',
      },
      retryAfter: '60',
    };
    expect(() => assertQuotaRefusal(result)).not.toThrow();
    for (const changed of [
      { ...result, status: 503 },
      { ...result, retryAfter: '0' },
      { ...result, body: { ...result.body, limit: 6 } },
      { ...result, body: { ...result.body, resource: 'other' } },
      { ...result, body: { ...result.body, reset_at: 'invalid' } },
      { ...result, body: { ...result.body, reset_at: '2026-11-14T01:00:00Z' } },
    ])
      expect(() => assertQuotaRefusal(changed)).toThrow();
  });
  it('uses server created_at UTC days and rejects per-day overshoot', () => {
    const rows = [
      ...Array.from({ length: 5 }, (_, i) => ({
        id: `old-${i}`,
        created_at: '2026-11-13T23:59:59Z',
      })),
      ...Array.from({ length: 5 }, (_, i) => ({
        id: `new-${i}`,
        created_at: '2026-11-14T00:00:00Z',
      })),
    ];
    expect(admissionDays(rows)).toEqual({ '2026-11-13': 5, '2026-11-14': 5 });
    expect(() =>
      admissionDays([...rows, { id: 'extra', created_at: rows[0]!.created_at }]),
    ).toThrow();
    expect(() => admissionDays([{ id: 'bad', created_at: 'invalid' }])).toThrow();
  });
  it('requires both control kinds, 100 percent success and p99 at most one second', () => {
    const samples = [
      { kind: 'stop' as const, status: 202, ms: 1000 },
      { kind: 'cancel' as const, status: 200, ms: 20 },
    ];
    expect(() => assertControls(samples)).not.toThrow();
    expect(() => assertControls([])).toThrow();
    expect(() => assertControls(samples.slice(0, 1))).toThrow();
    expect(() => assertControls([...samples, { kind: 'stop', status: 429, ms: 2 }])).toThrow();
    expect(() => assertControls([...samples, { kind: 'stop', status: 200, ms: 1001 }])).toThrow();
  });
  it('never presents missing samples as zero-error evidence', () => {
    const counts = newCounts();
    expect(metrics(counts)).toEqual({
      'visitor_overquota_rejects/attempts': null,
      idempotent_charge_duplicates: null,
      anonymous_side_effects: null,
      collateral_error_rate: null,
    });
    counts.overquotaAttempts = 45;
    counts.overquotaRejects = 44;
    counts.idempotencyChecks = 1;
    counts.idempotentChargeDuplicates = 2;
    counts.sideEffectChecks = 1;
    counts.anonymousSideEffects = 3;
    counts.collateralAttempts = 1000;
    counts.collateralErrors = 1;
    expect(metrics(counts)).toEqual({
      'visitor_overquota_rejects/attempts': 44 / 45,
      idempotent_charge_duplicates: 2,
      anonymous_side_effects: 3,
      collateral_error_rate: 0.001,
    });
  });
});

it('enforces the frozen fleet rate including its full initial burst allowance', () => {
  expect(() => assertAnonymousBudget(2400, 0, 1200, 2)).not.toThrow();
  expect(() => assertAnonymousBudget(4800, 60000, 1200, 2)).not.toThrow();
  expect(() => assertAnonymousBudget(4801, 60000, 1200, 2)).toThrow('frozen fleet');
});

it('blocks load fixtures where old per-network buckets can hide a missing fleet limiter', () => {
  const budget = {
    perPodPerMinute: 1200,
    maxServingPods: 2,
    networkPerMinute: 600,
    aboveRps: 200,
    belowRps: 5,
    durationSeconds: 30,
  };
  expect(() => assertLoadSeparation(budget, 2)).toThrow(/^BLOCKED:/);
  expect(() => assertLoadSeparation(budget, 6)).not.toThrow();
  expect(() => assertLoadSeparation({ ...budget, aboveRps: 50 }, 6)).toThrow(/^BLOCKED:/);
});
