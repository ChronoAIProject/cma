import { expect, it } from 'vitest';
import { sandboxLifecycle } from '../src/instances/lifecycle.js';

const base = { pause: { allowed: false }, revive: { allowed: false } };
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
