import { expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../src/index.js';
import { transport } from './resource-fixtures.js';
const page = {
  report_id: 'report',
  metric: 'cpu',
  unit: 'core-hours',
  items: [
    { subject: 'opaque', label: 'Alex', allocated_hours: 24, sandbox_count: 2, complete: true },
  ],
  next: 'signed',
  total_count: 400,
};
it('reads bounded administrator owner totals without changing a report', async () => {
  const test = transport();
  test.answer(page);
  expect(
    await test.client.adminUsageMetrics.ownerAllocations('report', {
      metric: 'cpu',
      limit: 50,
      cursor: 'previous',
    }),
  ).toEqual(page);
  expect(test.calls[0]?.url).toBe(
    'https://cma.example.test/api/v1/admin/usage/metrics/reports/report/owner-allocations?metric=cpu&limit=50&cursor=previous',
  );
  expect(test.calls[0]?.init.method).toBe('GET');
});
it('rejects non-admin requests, bad bounds, identity mismatches and invalid response data', async () => {
  const test = transport();
  expect(() => test.client.usageMetrics.ownerAllocations('report', { metric: 'cpu' })).toThrow(
    ValidationError,
  );
  for (const limit of [0, 201, 1.5])
    expect(() =>
      test.client.adminUsageMetrics.ownerAllocations('report', { metric: 'cpu', limit }),
    ).toThrow(ValidationError);
  expect(test.calls).toHaveLength(0);
  for (const change of [
    { report_id: 'other' },
    { metric: 'disk' },
    { unit: 'bytes' },
    { total_count: 1.5 },
    { items: [{ ...page.items[0], allocated_hours: -1 }] },
  ]) {
    test.answer({ ...page, ...change });
    await expect(
      test.client.adminUsageMetrics.ownerAllocations('report', { metric: 'cpu' }),
    ).rejects.toBeInstanceOf(ProtocolError);
  }
});
