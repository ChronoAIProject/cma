// @vitest-environment node
import { expect, it } from 'vitest';
import { ProtocolError } from '../index.js';
import { transport } from './resource-fixtures.js';

const full = {
  resource: 'sandbox',
  limit: 3,
  live: 5,
  available: 0,
  counts_by_status: { active: 1, paused: 1, failed: 1, rotating: 1, retiring: 1 },
  observed_at: '2026-11-19T12:00:00Z',
};

it('reads the authenticated owner quota including retained and retiring parents', async () => {
  const test = transport();
  test.answer(full);
  expect(await test.client.sandboxes.capacity()).toEqual(full);
  expect(test.calls[0]!.url).toBe('https://cma.example.test/api/v1/sandboxes/capacity');
});

it('reports settled retirements without charging them as live', async () => {
  const test = transport();
  const capacity = {
    ...full,
    live: 2,
    available: 1,
    counts_by_status: { active: 1, retiring: 1 },
    retiring_released: 2,
  };
  test.answer(capacity);
  expect(await test.client.sandboxes.capacity()).toEqual(capacity);
});

it.each([
  { live: 4 },
  { available: 1 },
  { observed_at: 'unknown' },
  { counts_by_status: { active: 4, retired: 1 } },
  { counts_by_status: { unknown: 5 } },
  { live: -1 },
])('refuses inconsistent capacity rather than inventing free slots: %j', async (invalid) => {
  const test = transport();
  test.answer({ ...full, ...invalid });
  await expect(test.client.sandboxes.capacity()).rejects.toBeInstanceOf(ProtocolError);
});

it.each([
  undefined,
  { code: 'sandbox_limit_reached', limit: 10, live: 10 },
  {
    code: 'sandbox_limit_reached',
    limit: 10,
    live: 10,
    quota_usage: { active: 7, retiring_charged: 3, retiring_released: 1 },
  },
])('reads old lifecycle errors and optional structured failures: %j', async (failure) => {
  const test = transport();
  const operation = {
    id: 'quota-operation',
    status: 'failed',
    phase: 'failed',
    error: 'Limit reached',
    events: [],
    ...(failure ? { failure } : {}),
  };
  test.answer(operation);
  expect(await test.client.agents.readLifecycle('quota-operation')).toEqual(operation);
});
