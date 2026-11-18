import { expect, test, type Page } from '@playwright/test';
import { LocalStartup, recorded } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { testBudget } from '../src/operator/spendTestFixtures';
import type { PlatformSettings, StoredPlatformSettings } from '@cma/client';

const cloud = { run_rate_units_per_day: 12, max_observation_age_seconds: 60 };
const rateLabel = 'Cloud run rate (accounting units per day)';
const ageLabel = 'Maximum cloud observation age (seconds)';

async function fixture(page: Page, baseURL: string, available?: boolean, retained = false) {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL);
  await new LocalUsageMetrics().session(page, ['use_own_sandboxes', 'configure_platform']);
  let current: StoredPlatformSettings = {
    ...recorded<StoredPlatformSettings>('admin.json', 'admin-settings-read'),
    revision: 4,
    model_budget_writes_available: true,
    model_request_budget_writes_available: true,
    platform_spend_writes_available: available,
  };
  current = {
    ...current,
    settings: {
      ...current.settings,
      model_budget: retained
        ? { ...testBudget, platform: { ...testBudget.platform!, cloud } }
        : testBudget,
    },
  };
  const writes: { expected_revision: number; settings: PlatformSettings }[] = [];
  await page.route('**/api/v1/admin/settings', async (route) => {
    if (route.request().method() === 'PUT') {
      const body = route.request().postDataJSON() as (typeof writes)[number];
      writes.push(body);
      expect(body.expected_revision).toBe(current.revision);
      current = { ...current, settings: body.settings, revision: current.revision + 1 };
    } else expect(route.request().method()).toBe('GET');
    await route.fulfill({ json: current });
  });
  return { boundary, writes };
}

test('LOCAL cloud controls validate the pair and save exact accounting units', async ({
  page,
  baseURL,
}, testInfo) => {
  const local = await fixture(page, baseURL!, true);
  await page.goto('/operations?tab=settings');
  await page.getByRole('button', { name: 'Configure cloud metering' }).click();
  const rate = page.getByLabel(rateLabel);
  const age = page.getByLabel(ageLabel);
  const save = page.getByRole('button', { name: 'Save settings' });
  await expect(rate).toHaveValue('');
  await expect(age).toHaveValue('');
  await rate.fill('12');
  await save.click();
  expect(local.writes).toHaveLength(0);
  await age.fill('60');
  await rate.fill('9007199254740991.1');
  await save.click();
  await expect(rate).toHaveAttribute('aria-invalid', 'true');
  await expect(rate).toHaveValue('9007199254740991.1');
  expect(local.writes).toHaveLength(0);
  await rate.fill('9007199254740991');
  await save.click();
  await expect(page.getByText('Settings saved at revision 5.')).toBeVisible();
  expect(local.writes[0]!.settings.model_budget).toEqual({
    ...testBudget,
    platform: {
      ...testBudget.platform,
      cloud: { ...cloud, run_rate_units_per_day: Number.MAX_SAFE_INTEGER },
    },
  });
  await rate.scrollIntoViewIfNeeded();
  await page.screenshot({ path: testInfo.outputPath('cloud-controls.png') });
  await page.getByRole('button', { name: 'Remove cloud metering' }).click();
  await save.click();
  await expect(page.getByText('Settings saved at revision 6.')).toBeVisible();
  expect(local.writes[1]!.settings.model_budget).toEqual(testBudget);
  expect(local.boundary.errors).toEqual([]);
  expect(local.boundary.writes).toEqual([]);
});

for (const available of [false, undefined]) {
  test(`LOCAL retained cloud blocks all settings saves with platform availability ${available}`, async ({
    page,
    baseURL,
  }) => {
    const local = await fixture(page, baseURL!, available, true);
    await page.goto('/operations?tab=settings');
    await expect(page.getByLabel(rateLabel)).toHaveValue('12');
    await expect(page.getByLabel(rateLabel)).toBeDisabled();
    await expect(page.getByLabel(ageLabel)).toBeDisabled();
    await expect(page.getByRole('button', { name: 'Remove cloud metering' })).toBeDisabled();
    await page.getByLabel('Agent log retention (days)').fill('45');
    await expect(page.getByRole('button', { name: 'Save settings' })).toBeDisabled();
    expect(local.writes).toHaveLength(0);
    expect(local.boundary.errors).toEqual([]);
  });
}
