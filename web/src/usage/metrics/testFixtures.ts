import { vi } from 'vitest';
import { asOf, metricRow, reportFixture, seriesFixture } from './fixtures.test-support';
export { asOf, metricRow, reportFixture, seriesFixture } from './fixtures.test-support';

export function metricsClient(admin = false) {
  return {
    options: vi.fn().mockResolvedValue({ items: [], next: null }),
    create: vi.fn().mockResolvedValue(reportFixture(admin)),
    report: vi.fn().mockResolvedValue(reportFixture(admin)),
    sandboxes: vi.fn().mockResolvedValue({
      report_id: 'report-one',
      as_of: asOf,
      items: [metricRow],
      next: 'page-two',
      total_count: 25001,
    }),
    sandbox: vi.fn().mockResolvedValue(metricRow),
    owners: vi.fn().mockResolvedValue({
      report_id: 'report-one',
      items: [{ subject: 'owner-1', display_name: 'Alex Chen' }],
      next: 'owners-two',
    }),
    series: vi
      .fn()
      .mockImplementation((id: string, options: { metric: 'cpu' | 'memory' | 'disk' }) =>
        Promise.resolve(seriesFixture(options.metric, id)),
      ),
  };
}
