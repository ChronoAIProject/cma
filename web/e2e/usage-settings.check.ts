import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';

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
    const toggle = page.getByRole('checkbox', { name: 'Show measured usage' });
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
