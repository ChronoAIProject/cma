import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { PerformanceObserver } from './ui-performance-observer';

const baseline = process.env.CMA_REQUEST_BASELINE === '1';
const duration = (baseline ? 5 : 12) * 60_000;
const paths = [
  '/api/v1/sandboxes/sbx_route',
  '/api/v1/workspaces/wks_route',
  '/api/v1/agents/agt_contract',
];

test.describe.configure({ mode: 'parallel' });
for (const surface of ['sandboxes/sbx_route', 'workspaces/wks_route']) {
  for (const tabs of baseline ? [1, 4] : [1]) {
    test(`LOCAL hierarchy ${surface} with ${tabs} tab(s)`, async ({ browser, baseURL }, info) => {
      test.setTimeout(duration + 60_000);
      const context = await browser.newContext();
      try {
        const runs = await Promise.all(
          Array.from({ length: tabs }, async () => {
            const page = await context.newPage();
            const fixture = new LocalStartup();
            fixture.sandbox = { ...fixture.sandbox, status: 'credential_rotating' };
            fixture.workspace = { ...fixture.workspace, status: 'ready' };
            await fixture.route(page, baseURL!);
            await page.route('**/api/v1/resource-access/workspace/wks_route', (route) =>
              route.fulfill({
                json: {
                  capabilities: ['view_details'],
                  owner: { kind: 'person', is_current_person: true },
                  revalidate_at: null,
                  policy_revision: 1,
                },
              }),
            );
            const observed = new PerformanceObserver(page);
            await observed.delay();
            await page.goto(`${baseURL}/${surface}`);
            await expect(page.locator('main h1')).toContainText(
              surface.startsWith('sandboxes') ? fixture.sandbox.name : fixture.workspace.name,
            );
            expect(await page.evaluate(() => document.hidden)).toBe(false);
            observed.begin('unchanged');
            return { page, fixture, observed };
          }),
        );
        await runs[0]!.page.waitForTimeout(duration);
        for (const [index, { fixture, observed }] of runs.entries()) {
          const from = observed.marks.unchangedStart!;
          const summaries = paths.map((path) => {
            const all = observed.requests.filter((read) => read.path === path);
            const reads = all.filter(
              (read) => read.startMs >= from && read.startMs < from + duration,
            );
            const gaps = reads.slice(1).map((read, i) => read.startMs - reads[i]!.startMs);
            const idle = reads
              .slice(1)
              .map((read, i) => read.startMs - (reads[i]!.endMs ?? Infinity));
            const overlaps = all.filter((read, i) =>
              all.slice(0, i).some((prior) => (prior.endMs ?? Infinity) > read.startMs),
            ).length;
            const minutes = Array.from(
              { length: duration / 60_000 },
              (_, minute) =>
                reads.filter(
                  (read) =>
                    read.startMs >= from + minute * 60_000 &&
                    read.startMs < from + (minute + 1) * 60_000,
                ).length,
            );
            return { path, count: reads.length, minutes, gaps, idle, overlaps };
          });
          await info.attach(`tab-${index + 1}-hierarchy`, {
            body: JSON.stringify({ baseline, duration, summaries, requests: observed.requests }),
            contentType: 'application/json',
          });
          for (const summary of summaries) {
            if (!baseline) {
              // Full Console totals include up to four sidebar reads (#1803)
              // and two access revalidations per minute. They use the same URL;
              // keep them in the evidence, including cross-consumer overlaps.
              // hierarchyPolling.test.tsx enforces the page-only 14/5/3 bounds
              // and zero overlap. Full-route issue acceptance remains separate.
              expect.soft(summary.minutes[0], summary.path).toBeLessThanOrEqual(14 + 6);
              for (const count of summary.minutes.slice(2))
                expect.soft(count).toBeLessThanOrEqual(5 + 6);
              for (const count of summary.minutes.slice(10))
                expect.soft(count).toBeLessThanOrEqual(3 + 6);
            }
          }
          if (!baseline && surface.startsWith('sandboxes')) {
            for (const [child, summary] of summaries.slice(1).entries())
              expect(
                summary.minutes.slice(0, 5).reduce((sum, value) => sum + value, 0),
              ).toBeLessThanOrEqual(child === 0 ? 41 : 21);
          }
          expect(fixture.writes).toEqual([]);
          expect(fixture.errors).toEqual([]);
          expect(fixture.unexpected).toEqual([]);
        }
      } finally {
        await context.close();
      }
    });
  }
}
