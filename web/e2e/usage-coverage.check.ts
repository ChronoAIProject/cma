import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { metricRow } from '../src/usage/metrics/fixtures.test-support';

for (const admin of [false, true]) {
  for (const width of [1440, 390]) {
    test(`LOCAL coverage and projection agree across usage views admin=${admin} width=${width}`, async ({
      page,
      baseURL,
    }, info) => {
      const boundary = new LocalStartup();
      const metrics = new LocalUsageMetrics();
      metrics.ownerCount = 6;
      for (let index = 0; index < 6; index += 1)
        metrics.ownerOverrides.set(index, { complete: index !== 5 });
      await boundary.route(page, baseURL!);
      await metrics.route(page);
      await metrics.session(
        page,
        admin ? ['use_own_sandboxes', 'read_all_sandboxes'] : ['use_own_sandboxes'],
      );
      await page.setViewportSize({ width, height: 1000 });
      let loaded = false;
      for (const [complete, unknown, label] of [
        [false, 0, 'Coverage unverified'],
        [false, 3600, 'Missing intervals confirmed'],
        [true, 0, 'Coverage verified'],
      ] as const) {
        for (const projected of [0, 120]) {
          metrics.presentationRows = [
            {
              ...metricRow,
              allocated_period: {
                ...metricRow.allocated_period,
                complete,
                unknown_seconds: unknown,
                projected_tail_seconds: projected,
              },
            },
          ];
          if (loaded)
            await page.getByRole('button', { name: 'Refresh usage', exact: true }).click();
          else await page.goto(admin ? '/operations?tab=sandboxes' : '/usage');
          loaded = true;
          const surface = admin
            ? page.getByRole('region', { name: 'Sandbox resource dashboard' })
            : page.getByRole('dialog', { name: 'Your usage' });
          const summary = surface.getByRole('region', {
            name: 'Allocated resource-hours',
            exact: true,
          });
          const chart = surface.getByRole('region', { name: 'Allocation comparison chart' });
          const inventory = surface.getByRole('table', { name: 'Sandboxes and allocated usage' });
          await expect(summary.locator('.badge')).toHaveText(label);
          await expect(summary).toContainText(
            'Resource-hours describe allocation; billing is separate',
          );
          for (const [metric, total] of [
            ['CPU (core-hours)', '24'],
            ['Memory (GiB-hours)', '48'],
            ['Disk (GiB-hours)', '480'],
          ] as const) {
            await expect(
              summary
                .getByRole('term')
                .filter({ hasText: metric })
                .locator('..')
                .locator(':scope > dd'),
            ).toHaveText(total);
          }
          await expect(inventory.locator('.usage-inventory-quantity')).toHaveText([
            '24',
            '48',
            '480',
          ]);
          await expect(chart.getByLabel('Plotted Sandboxes coverage')).toContainText(label);
          await expect(inventory.getByText(label, { exact: true })).toBeVisible();
          for (const view of [summary, chart, inventory]) {
            await expect(view).not.toContainText(/Partial history|Complete history/);
            if (!complete && unknown === 0) {
              await expect(view).toContainText('No missing interval has been established');
              await expect(view).not.toContainText(/Missing intervals are excluded|zero usage/);
            }
            if (projected)
              await expect(view).toContainText('Projection from last known state: 2m (estimated)');
            else await expect(view).not.toContainText('(estimated)');
          }
          const data = chart.locator('.usage-chart-data');
          if ((await data.getAttribute('open')) === null) await data.locator('summary').click();
          const table = chart.getByRole('table', { name: 'Allocation chart data' });
          await expect(table.getByText(label, { exact: true })).toBeVisible();
          const history = summary.locator('.usage-allocation-history');
          if ((await history.getAttribute('open')) === null)
            await history.locator('summary').click();
          await expect(
            history
              .locator('dt')
              .filter({ hasText: 'Missing intervals' })
              .locator('..')
              .locator('dd'),
          ).toHaveText(unknown ? '1h' : complete ? 'None' : 'None confirmed');
          const rowDetails = inventory.locator('.usage-allocation-coverage details');
          if ((await rowDetails.getAttribute('open')) === null) {
            await rowDetails.locator('summary').focus();
            await page.keyboard.press('Enter');
          }
          await expect(rowDetails.locator('p')).toBeVisible();
          await expect(inventory.locator('.badge__word').filter({ hasText: label })).toBeVisible();
          expect(
            await inventory
              .locator('.badge__word')
              .filter({ hasText: label })
              .evaluate(
                (node) =>
                  node.scrollWidth <= node.clientWidth + 1 &&
                  node.scrollHeight <= node.clientHeight + 1,
              ),
          ).toBe(true);
          await chart.scrollIntoViewIfNeeded();
          await page.screenshot({
            path: info.outputPath(`coverage-${complete}-${unknown}-${projected}-${width}.png`),
          });
        }
      }
      if (admin) {
        const chart = page.getByRole('region', { name: 'Allocation comparison chart' });
        metrics.presentationRows = [
          {
            ...metricRow,
            allocated_period: {
              ...metricRow.allocated_period,
              complete: false,
              unknown_seconds: 3600,
              projected_tail_seconds: 0,
            },
          },
        ];
        await page.getByRole('button', { name: 'Refresh usage', exact: true }).click();
        const reportCoverage = chart.getByRole('note', { name: 'Report allocation coverage' });
        await expect(reportCoverage).toContainText('Confirmed missing duration: 1h');
        await chart.getByRole('button', { name: 'Allocation chart grouping' }).click();
        await page.getByRole('menuitemradio', { name: 'By owner', exact: true }).click();
        const note = chart.getByRole('note', { name: 'Selected owners coverage' });
        await expect(note).toContainText('Coverage unverified');
        await expect(note).not.toContainText('No missing interval has been established');
        await expect(reportCoverage).toContainText('Confirmed missing duration: 1h');
        await expect(note).toContainText(
          'Missing-interval durations are unavailable for this scope',
        );
        await expect(note).toContainText('Projection details are unavailable for this scope');
        await expect(note).not.toContainText(
          /None confirmed|Missing intervals confirmed|estimated/,
        );
        await chart.getByText('View owner allocation chart data').click();
        const table = chart.getByRole('table', { name: 'Owner allocation chart data' });
        await expect(table.getByText('Coverage unverified', { exact: true })).toHaveCount(1);
        await expect(table.getByText('Coverage verified', { exact: true })).toHaveCount(5);
        await chart.getByRole('button', { name: 'Owners shown' }).click();
        await page.getByRole('menuitemradio', { name: 'Top 5', exact: true }).click();
        await expect(note).toContainText('Coverage verified');
        await expect(note).not.toContainText('Coverage unverified');
        await expect(note).toContainText('Projection details are unavailable for this scope');
        await chart.scrollIntoViewIfNeeded();
        await page.screenshot({ path: info.outputPath(`owner-coverage-${width}.png`) });
      }
      expect(boundary.writes).toEqual([]);
      expect(boundary.errors).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
    });
  }
}
