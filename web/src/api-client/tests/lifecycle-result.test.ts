// @vitest-environment node
import { readFileSync } from 'node:fs';
import { expect, it } from 'vitest';
import { sandboxLifecycle } from '../instances/lifecycle.js';

const base = { pause: { allowed: false }, revive: { allowed: false } };
it('accepts the additive parked code with the existing failed result and eligibility contract', () => {
  const result = {
    state: 'failed',
    code: 'runtime_update_parked',
    continuing: false,
    operation_id: 'original',
    started_at: '2026-11-16T00:00:00Z',
    result_due_at: '2026-11-16T00:01:30Z',
    actions: ['observe'],
  };
  expect(sandboxLifecycle({ ...base, result }).result).toEqual(result);
  expect(sandboxLifecycle({ ...base, result }).revive.allowed).toBe(false);
});
it('accepts older servers and decodes bounded lifecycle results without changing eligibility', () => {
  expect(sandboxLifecycle(base).result).toBeUndefined();
  const result = {
    state: 'recoverable_pending',
    code: 'lifecycle_wait_exceeded',
    operation_id: 'original',
    started_at: '2026-11-16T00:00:00Z',
    result_due_at: '2026-11-16T00:01:30Z',
    continuing: true,
    actions: ['observe', 'retire'],
    retry_after_seconds: 5,
  };
  expect(sandboxLifecycle({ ...base, result }).result).toEqual(result);
  expect(sandboxLifecycle({ ...base, result }).revive.allowed).toBe(false);
  expect(() => sandboxLifecycle({ ...base, result: { ...result, continuing: 'yes' } })).toThrow();
  expect(() =>
    sandboxLifecycle({ ...base, result: { ...result, actions: ['start_fresh'] } }),
  ).toThrow();
});

it('omits polling guidance for a terminal failure and accepts its absence from older servers', () => {
  const result = JSON.parse(
    readFileSync(
      new URL(
        '../../../../api/crates/layer1-public/tests/fixtures/lifecycle-result-failed.json',
        import.meta.url,
      ),
      'utf8',
    ),
  ) as Record<string, unknown>;
  expect(sandboxLifecycle({ ...base, result }).result).toEqual(result);
  expect(sandboxLifecycle({ ...base, result }).result?.retry_after_seconds).toBeUndefined();
  expect(() =>
    sandboxLifecycle({ ...base, result: { ...result, retry_after_seconds: 0 } }),
  ).toThrow();
});

it('decodes a retained failure without inventing missing admission identity or timestamps', () => {
  const result = {
    state: 'failed',
    code: 'lifecycle_failed_admission_unavailable',
    continuing: false,
    actions: ['observe', 'retire'],
  };
  const decoded = sandboxLifecycle({ ...base, result }).result;
  expect(decoded).toEqual(result);
  expect(decoded?.operation_id).toBeUndefined();
  expect(decoded?.started_at).toBeUndefined();
  expect(decoded?.result_due_at).toBeUndefined();
  expect(decoded?.retry_after_seconds).toBeUndefined();
});
