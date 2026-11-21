import type { Page } from '@playwright/test';
import { asOf, reportFixture } from '../src/usage/metrics/fixtures.test-support';
import { LocalUsageMetrics } from './usage-metrics.fixture';

/** Ready retained views for image tests; legacy compatibility uses LocalUsageMetrics. */
export class LocalRetainedUsageMetrics extends LocalUsageMetrics {
  readonly views: URL[] = [];
  override async route(page: Page) {
    await super.route(page);
    await page.route('**/api/v1/**usage/metrics/view**', async (route) => {
      const url = new URL(route.request().url());
      this.views.push(url);
      const admin = url.pathname.includes('/admin/');
      const window = url.searchParams.get('window');
      const seconds = window === '24h' ? 86400 : window === '7d' ? 604800 : 2592000;
      if (!['24h', '7d', '30d'].includes(window ?? ''))
        throw new Error('Image retained-view fixture requires a preset range.');
      const id = `retained-${admin ? 'admin' : 'owner'}-${window}`;
      const base = reportFixture(admin, id);
      const range = { from: asOf - seconds, to: asOf };
      const report = {
        ...base,
        scope: { principal: 'local-owner', admin },
        query: {
          ...base.query,
          ...range,
          step_seconds: window === '24h' ? 60 : window === '7d' ? 600 : 3600,
          filters: {
            ...base.query.filters,
            ...JSON.parse(url.searchParams.get('filters') ?? '{}'),
          },
        },
        effective_range: range,
      };
      this.reports.set(id, report);
      await route.fulfill({
        json: {
          schema_version: 1,
          window,
          requested_range: range,
          report,
          generated_at: asOf,
          stale: false,
          refresh: { state: 'idle', report_id: null, retry_after_seconds: 0 },
        },
      });
    });
  }
}
