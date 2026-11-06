import { expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../src/index.js';
import { transport } from './resource-fixtures.js';

it('loads administrator options without creating a report and preserves opaque values', async () => {
  const test = transport();
  const page = {
    items: [{ value: 'owner-a', label: 'Alex Chen', disambiguator: 'Joined 2026-11-01' }],
    next: 'next-page',
  };
  test.answer(page);
  expect(
    await test.client.adminUsageMetrics.options({ kind: 'owner', search: 'Alex', limit: 50 }),
  ).toEqual(page);
  expect(test.calls.at(-1)?.url).toBe(
    'https://cma.example.test/api/v1/admin/usage/metrics/options',
  );
  test.answer(page);
  await test.client.adminUsageMetrics.options({
    kind: 'sandbox',
    owner: 'owner-a',
    cursor: 'next-page',
  });
  expect(JSON.parse(String(test.calls.at(-1)?.init.body))).toEqual({
    kind: 'sandbox',
    owner: 'owner-a',
    cursor: 'next-page',
  });
  test.answer(page);
  await test.client.adminUsageMetrics.options({ kind: 'owner', selected: 'owner-a', limit: 1 });
  expect(JSON.parse(String(test.calls.at(-1)?.init.body))).toEqual({
    kind: 'owner',
    selected: 'owner-a',
    limit: 1,
  });
  expect(test.calls).toHaveLength(3);
});
it('rejects malformed options and forbids owner-scope discovery', async () => {
  const test = transport();
  expect(() => test.client.usageMetrics.options({ kind: 'owner' })).toThrow(ValidationError);
  for (const limit of [0, 51, 1.5])
    expect(() => test.client.adminUsageMetrics.options({ kind: 'sandbox', limit })).toThrow(
      ValidationError,
    );
  expect(test.calls).toHaveLength(0);
  test.answer({ items: [{ value: 'owner-a' }], next: null });
  await expect(test.client.adminUsageMetrics.options({ kind: 'owner' })).rejects.toBeInstanceOf(
    ProtocolError,
  );
});

it('accepts plain labels without duplicate context and rejects malformed context', async () => {
  const test = transport();
  const page = { items: [{ value: 'owner-a', label: 'Alex Chen' }], next: null };
  test.answer(page);
  expect(await test.client.adminUsageMetrics.options({ kind: 'owner' })).toEqual(page);
  test.answer({ items: [{ value: 'owner-a', label: 'Alex Chen', disambiguator: 42 }], next: null });
  await expect(test.client.adminUsageMetrics.options({ kind: 'owner' })).rejects.toBeInstanceOf(
    ProtocolError,
  );
});
