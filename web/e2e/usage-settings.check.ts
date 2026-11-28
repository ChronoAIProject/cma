import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { LocalUsageFilterOptions } from './usage-filter-options.fixture';
import type { UsageMetricQuery } from '@cma/client';
import { reportFixture } from '../src/usage/metrics/fixtures.test-support';

test('LOCAL admin saved visibility controls measured usage without enabling collection', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  const metrics = new LocalUsageMetrics();
  await boundary.route(page, baseURL!);
  await metrics.route(page);
  await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes', 'configure_platform']);
  let revision = 4;
  let settings = {
    defaults: {
      sandbox_profile: { id: 'sbp_default', revision: 1 },
      workspace_profile: { id: 'wkp_default', revision: 2 },
      agent_profile: { id: 'agp_default', revision: 3 },
    },
    inactivity: { auto_pause_enabled: false, pause_after_seconds: 900 },
    retention: { agent_log_days: 30 },
    limits: {
      max_sandboxes_per_user: 10,
      visitor_max_sandboxes: 1,
      visitor_max_launches_per_day: 5,
    },
    triggers: { enabled: true, orchestration_paused: false },
    marketplace: { enabled: true },
    usage_reporting: { measured_visible: false },
  };
  const writes: boolean[] = [];
  await page.route('**/api/v1/admin/settings', async (route) => {
    if (route.request().method() === 'PUT') {
      const input = route.request().postDataJSON() as {
        expected_revision: number;
        settings: typeof settings;
      };
      expect(input.expected_revision).toBe(revision);
      expect(input.settings.inactivity).toEqual(settings.inactivity);
      expect(input.settings.triggers).toEqual(settings.triggers);
      settings = input.settings;
      metrics.measuredVisible = settings.usage_reporting.measured_visible;
      writes.push(metrics.measuredVisible);
      revision += 1;
    } else expect(route.request().method()).toBe('GET');
    await route.fulfill({ json: { revision, settings } });
  });
  const usagePath =
    process.env.CMA_E2E_USAGE_SURFACE === 'account' ? '/usage' : '/operations?tab=sandboxes';
  for (const enabled of [true, false]) {
    await page.goto('/operations?tab=settings');
    await page.getByRole('tab', { name: 'Platform settings' }).click();
    const card = page.getByRole('region', { name: 'Measured usage', exact: true });
    await card.getByRole('button', { name: 'Edit Measured usage' }).click();
    const toggle = card.getByRole('checkbox', { name: 'Show measured usage' });
    await expect(toggle).toBeChecked({ checked: !enabled });
    await toggle.click();
    const savedRevision = revision + 1;
    await page.getByRole('button', { name: 'Save settings' }).click();
    await expect(page.getByText(`Settings saved at revision ${savedRevision}.`)).toBeVisible();
    await page.goto(usagePath);
    await expect(page.getByRole('button', { name: 'Research Sandbox', exact: true })).toBeVisible();
    await expect(page.getByRole('region', { name: 'Allocated resource-hours' })).toBeVisible();
    if (enabled)
      await expect(page.getByRole('region', { name: 'Measured resource charts' })).toBeVisible();
    else {
      await expect(page.getByRole('region', { name: 'Allocated capacity now' })).toBeVisible();
      await expect(page.getByRole('region', { name: 'Measured resource charts' })).toHaveCount(0);
      const seriesCount = metrics.reads.filter((url) => url.pathname.endsWith('/series')).length;
      await page.getByRole('button', { name: 'Research Sandbox', exact: true }).click();
      const detail = page.getByRole('region', { name: 'Sandbox usage details' });
      await expect(detail.getByRole('region', { name: 'Allocated capacity now' })).toBeVisible();
      expect(metrics.reads.filter((url) => url.pathname.endsWith('/series'))).toHaveLength(
        seriesCount,
      );
    }
  }
  expect(writes).toEqual([true, false]);
  expect(boundary.writes).toEqual([]);
});

for (const admin of [false, true]) {
  test(`LOCAL ${admin ? 'Operations' : 'owner'} range errors follow the draft until explicit Apply`, async ({
    page,
    baseURL,
  }) => {
    const boundary = new LocalStartup();
    const metrics = new LocalUsageMetrics();
    await boundary.route(page, baseURL!);
    await metrics.route(page);
    await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
    await new LocalUsageFilterOptions().route(page);
    await page.route('**/api/v1/**usage/metrics/reports', async (route) => {
      expect(route.request().method()).toBe('POST');
      const input = route.request().postDataJSON() as UsageMetricQuery;
      expect(input.from).toBeDefined();
      expect(input.to).toBeDefined();
      const id = `draft-report-${metrics.creates.length + 1}`;
      const base = reportFixture(admin, id);
      const report = {
        ...base,
        scope: { principal: 'local-owner', admin },
        query: { ...base.query, ...input, filters: { ...base.query.filters, ...input.filters } },
        effective_range: { from: input.from!, to: input.to! },
      };
      metrics.creates.push({ admin, input, id });
      metrics.reports.set(id, report);
      await route.fulfill({ json: report });
    });
    const requests: string[] = [];
    page.on('request', (request) => {
      if (request.url().includes('/usage/metrics/')) requests.push(request.url());
    });
    await page.goto(admin ? '/operations?tab=sandboxes' : '/usage');
    const dashboard = page.locator('.usage-metrics');
    await expect(dashboard).toHaveAttribute('data-usage-state', 'committed');
    await expect(page.getByRole('button', { name: 'Research Sandbox', exact: true })).toBeVisible();
    await page.waitForLoadState('networkidle');
    const committedUrl = page.url();
    const period = page.getByText(/^Allocation period:/);
    const committedPeriod = await period.innerText();
    const committedRequests = [...requests];
    const range = page.getByRole('button', { name: 'Usage time range' });
    const apply = page.getByRole('button', { name: 'Apply usage filters' });
    const form = page.getByRole('form', { name: 'Usage filters' });
    const pending = form.getByText('Changes apply when you select Apply filters.');
    await range.click();
    await page.getByRole('menuitemradio', { name: 'Custom (up to 30 days)', exact: true }).click();
    await page.getByLabel('Usage range from').fill('2020-01-03T00:00');
    await page.getByLabel('Usage range to').fill('2020-01-01T00:00');
    await apply.click();
    await expect(form.getByRole('alert')).toHaveText(
      'Choose a valid range of up to 30 days, ending no later than now.',
    );
    await range.click();
    await page.getByRole('menuitemradio', { name: 'Last hour', exact: true }).click();
    await expect(range).toHaveText('Last hour');
    await expect(form.getByRole('alert')).toHaveCount(0);
    await expect(pending).toBeVisible();
    await expect(page).toHaveURL(committedUrl);
    await expect(period).toHaveText(committedPeriod);
    await expect(dashboard).toHaveAttribute('data-usage-requested-range', '24h');
    await expect(dashboard).toHaveAttribute('data-usage-state', 'committed');
    await page.evaluate(
      () =>
        new Promise<void>((resolve) =>
          requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
        ),
    );
    expect(requests).toEqual(committedRequests);
    expect(metrics.creates).toHaveLength(1);
    await apply.click();
    await expect(dashboard).toHaveAttribute('data-usage-requested-range', '1h');
    await expect(dashboard).toHaveAttribute('data-usage-state', 'committed');
    await expect(period).not.toHaveText(committedPeriod);
    await expect(pending).toHaveCount(0);
    expect(new URL(page.url()).searchParams.get('range')).toBe('1h');
    expect(metrics.creates).toHaveLength(2);
    const applied = metrics.creates[1]!.input;
    expect(Number(applied.to) - Number(applied.from)).toBe(3600);
    expect(boundary.errors).toEqual([]);
    expect(boundary.writes).toEqual([]);
  });
}
