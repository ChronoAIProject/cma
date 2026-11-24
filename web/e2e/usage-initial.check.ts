import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { asOf, reportFixture } from '../src/usage/metrics/fixtures.test-support';

for (const admin of [false, true]) {
  for (const refused of [false, true]) {
    test(`LOCAL initial usage retains admission across replicas admin=${admin} refused=${refused}`, async ({
      page,
      baseURL,
    }) => {
      const boundary = new LocalStartup();
      const metrics = new LocalUsageMetrics();
      await boundary.route(page, baseURL!);
      await metrics.route(page);
      await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
      const id = 'umr_00000000000000000000000000000001';
      const report = { ...reportFixture(admin, id), scope: { principal: 'local-owner', admin } };
      metrics.reports.set(id, report);
      const pending = {
        ...report,
        status: 'pending',
        report_complete: false,
        summary: null,
        retry_after_seconds: 2,
      };
      const initialReads: { path: string; at: number }[] = [];
      const legacyReads: number[] = [];
      const selectedLatest: string[] = [];
      const base = `/api/v1/${admin ? 'admin/' : ''}usage/metrics`;
      await page.route('**/api/v1/**usage/metrics/initial/**', async (route) => {
        const request = route.request();
        const path = new URL(request.url()).pathname;
        expect(request.method()).toBe('POST');
        initialReads.push({ path, at: Date.now() });
        if (path === `${base}/initial/view`) {
          expect(request.postDataJSON().selection.window).toBe('24h');
          return route.fulfill({
            status: 202,
            headers: { 'Retry-After': '2' },
            json: {
              view: {
                schema_version: 1,
                window: '24h',
                requested_range: { from: asOf - 86400, to: asOf },
                report: pending,
                generated_at: null,
                stale: false,
                refresh: { state: 'pending', report_id: id, retry_after_seconds: 2 },
              },
            },
          });
        }
        expect(path).toBe(`${base}/initial/reports/${id}`);
        const status = refused ? 403 : 501;
        return route.fulfill({
          status,
          json: {
            status,
            title: refused ? 'Access refused' : 'Route unavailable',
            code: refused ? 'forbidden' : 'report_view_disabled',
          },
        });
      });
      await page.route('**/api/v1/**usage/metrics/view**', async (route) => {
        const url = new URL(route.request().url());
        if (url.searchParams.get('window') === '24h') selectedLatest.push(url.pathname);
        return route.fulfill({
          status: 501,
          json: { status: 501, title: 'Route unavailable', code: 'report_view_disabled' },
        });
      });
      await page.route(`**${base}/reports/${id}`, async (route) => {
        expect(route.request().method()).toBe('GET');
        legacyReads.push(Date.now());
        return route.fulfill({ json: legacyReads.length === 1 ? pending : report });
      });
      await page.goto(admin ? '/operations?tab=sandboxes' : '/usage');
      const dashboard = page.locator('.usage-metrics');
      await expect(dashboard).toHaveAttribute('data-usage-state', 'pending');
      await expect(dashboard).toHaveAttribute('data-usage-state', refused ? 'error' : 'committed', {
        timeout: 10000,
      });
      expect(initialReads.map(({ path }) => path)).toEqual([
        `${base}/initial/view`,
        `${base}/initial/reports/${id}`,
      ]);
      expect(initialReads[1]!.at - initialReads[0]!.at).toBeGreaterThanOrEqual(2000);
      expect(selectedLatest).toEqual([]);
      expect(metrics.creates).toEqual([]);
      if (refused) {
        await expect(
          dashboard.getByText('Usage access unavailable', { exact: true }),
        ).toBeVisible();
        expect(legacyReads).toEqual([]);
        expect(metrics.reads).toEqual([]);
      } else {
        await expect(
          dashboard.getByRole('button', { name: 'Research Sandbox', exact: true }),
        ).toBeVisible();
        expect(legacyReads).toHaveLength(2);
        expect(legacyReads[1]! - legacyReads[0]!).toBeGreaterThanOrEqual(2000);
        expect(metrics.reads.length).toBeGreaterThan(0);
        expect(
          metrics.reads.every((url) => url.pathname.startsWith(`${base}/reports/${id}/`)),
        ).toBe(true);
      }
      expect(boundary.writes).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.errors).toEqual([]);
    });
  }
}
