import { describe, expect, it } from 'vitest';
import { defaultSelection, displayQuantity, metricQuery, metricValue } from './model';
import { readUsageSelection, usageSearch } from './selection';

const now = Date.parse('2026-11-05T05:15:27Z');
describe('measured usage selection', () => {
  it.each(['1h', '24h', '7d', '30d'] as const)(
    'keeps %s within the server bucket ceiling',
    (range) => {
      const query = metricQuery({ ...defaultSelection, range }, true, now);
      expect(query.to! % query.step_seconds!).toBe(0);
      expect((query.to! - query.from!) / query.step_seconds!).toBeLessThanOrEqual(2016);
      expect(query.to!).toBeLessThanOrEqual(now / 1000);
    },
  );
  it('never sends an owner filter from account Usage', () => {
    const query = metricQuery({ ...defaultSelection, owner: 'another-owner' }, false, now);
    expect(query.filters).not.toHaveProperty('owner');
  });
  it.each([
    { from: '', to: '' },
    { from: '2026-09-01T00:00:00Z', to: '2026-11-01T00:00:00Z' },
    { from: '2026-11-04T00:00:00Z', to: '2026-11-06T00:00:00Z' },
    { from: '2026-11-04T00:00:00Z', to: '2026-11-03T00:00:00Z' },
  ])('rejects an invalid custom range', (custom) => {
    expect(() =>
      metricQuery({ ...defaultSelection, range: 'custom', ...custom }, true, now),
    ).toThrow('30 days');
  });
  it('asks the server to choose valid custom boundary buckets', () => {
    const query = metricQuery(
      {
        ...defaultSelection,
        range: 'custom',
        from: '2026-10-30T00:00:01Z',
        to: '2026-11-05T00:00:02Z',
      },
      true,
      now,
    );
    expect(query).not.toHaveProperty('step_seconds');
  });
  it('preserves unknown quantities and exact large integer strings', () => {
    expect(metricValue(null, 'disk')).toBe('Unavailable');
    expect(metricValue(0, 'cpu')).toBe('0 cores');
    expect(displayQuantity('9007199254740993')).toBe('9,007,199,254,740,993');
  });
  it('round trips filters and sort while rejecting administrator fields for owners', () => {
    const selection = {
      ...defaultSelection,
      owner: 'owner-2',
      sandbox: 'sbx-2',
      range: '7d' as const,
      sort: 'cpu_mean_cores' as const,
      order: 'asc' as const,
    };
    expect(readUsageSelection(usageSearch(selection, true), true)).toEqual(selection);
    expect(readUsageSelection('?owner=foreign&sort=owner&range=wrong&status=wrong', false)).toEqual(
      defaultSelection,
    );
  });
});
