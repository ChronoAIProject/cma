import { test, expect, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { testBudget } from '../src/operator/spendTestFixtures';
import type { PlatformSettings } from '@chronoai/cma-sdk';

async function fixture(page: Page, baseURL: string) {
  const boundary = new LocalStartup();
  const metrics = new LocalUsageMetrics();
  await boundary.route(page, baseURL);
  await metrics.session(page, ['use_own_sandboxes', 'configure_platform', 'read_activity_log']);
  let revision = 4;
  let settings: PlatformSettings = {
    defaults: {
      sandbox_profile: { id: 'sbp_fixture', revision: 1 },
      workspace_profile: { id: 'wkp_fixture', revision: 1 },
      agent_profile: { id: 'agp_fixture', revision: 1 },
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
    model_budget: structuredClone(testBudget),
    future: { retained: true },
  };
  const writes: { expected_revision: number; settings: PlatformSettings }[] = [];
  let failure = 0;
  await page.route('**/api/v1/admin/settings', async (route) => {
    if (route.request().method() === 'PUT') {
      const body = route.request().postDataJSON() as (typeof writes)[number];
      writes.push(body);
      if (failure)
        return route.fulfill({
          status: failure,
          json: {
            status: failure,
            title: 'Settings refused',
            code: failure === 409 ? 'settings_conflict' : 'invalid_request',
          },
        });
      expect(body.expected_revision).toBe(revision);
      expect(body.settings.future).toEqual(settings.future);
      settings = body.settings;
      revision += 1;
    } else expect(route.request().method()).toBe('GET');
    await route.fulfill({ json: { revision, settings } });
  });
  await page.route('**/api/v1/admin/activity*', (route) =>
    route.fulfill({
      json: {
        items: [
          {
            id: 'fixture-activity',
            at: '2026-11-10T00:00:00Z',
            actor: 'fixture-admin',
            actor_name: 'Test administrator',
            action: 'configure',
            target: { kind: 'resource', id: 'platform_settings' },
            outcome: { status: 'succeeded' },
            detail: { revision: String(revision) },
          },
        ],
        next: null,
      },
    }),
  );
  return {
    boundary,
    writes,
    setFailure: (status: number) => {
      failure = status;
    },
    advance: () => {
      revision = 8;
      settings = {
        ...settings,
        retention: { agent_log_days: 45 },
        model_budget: {
          ...testBudget,
          owners: [{ owner: 'fixture-owner', policy: testBudget.default_policy }],
          platform: { ...testBudget.platform, warning_percent: 50 },
        },
      };
    },
  };
}

test('LOCAL admin spend save, disable and exact-number validation', async ({ page, baseURL }) => {
  const local = await fixture(page, baseURL!);
  await page.goto('/operations?tab=settings');
  await expect(page.getByRole('heading', { name: 'Spend controls' })).toBeVisible();
  await expect(page.getByText(/Last platform settings change: Test administrator/)).toBeVisible();
  await page.getByLabel('Enable spend controls').check();
  const cap = page.getByLabel('Platform period cap (accounting units)', { exact: true });
  await cap.fill('9007199254740991.1');
  await page.getByRole('button', { name: 'Save settings' }).click();
  await expect(cap).toHaveAttribute('aria-invalid', 'true');
  expect(local.writes).toHaveLength(0);
  await cap.fill(String(testBudget.platform!.period_cap));
  await page.getByLabel('Per-turn cap (accounting units)', { exact: true }).fill('80');
  await page.getByRole('button', { name: 'Save settings' }).click();
  await expect(page.getByText('Settings saved at revision 5.')).toBeVisible();
  await page.getByLabel('Enable spend controls').uncheck();
  await page.getByRole('button', { name: 'Save settings' }).click();
  await expect(page.getByText('Settings saved at revision 6.')).toBeVisible();
  expect(local.writes[1]!.settings.model_budget).toEqual({
    ...testBudget,
    default_policy: { ...testBudget.default_policy, turn_limit: 80 },
  });
  expect(local.boundary.writes).toEqual([]);
  expect(local.boundary.errors).toEqual([]);
});

test('LOCAL conflicts require review and server validation errors retain the draft', async ({
  page,
  baseURL,
}) => {
  const local = await fixture(page, baseURL!);
  await page.goto('/operations?tab=settings');
  await page.getByLabel('Per-turn cap (accounting units)', { exact: true }).fill('80');
  local.advance();
  local.setFailure(409);
  await page.getByRole('button', { name: 'Save settings' }).click();
  await expect(page.getByText(/The current revision is 8/)).toBeVisible();
  await expect(page.getByRole('button', { name: 'Save settings' })).toBeDisabled();
  await page.getByRole('button', { name: 'Review retained draft' }).click();
  await expect(page.getByLabel('Warning threshold (%)')).toHaveValue('50');
  local.setFailure(422);
  await page.getByRole('button', { name: 'Save settings' }).click();
  await expect(page.getByRole('alert')).toContainText('Review the settings fields');
  await expect(page.getByLabel('Per-turn cap (accounting units)', { exact: true })).toHaveValue(
    '80',
  );
  local.setFailure(0);
  await page.getByRole('button', { name: 'Save settings' }).click();
  await expect(page.getByText('Settings saved at revision 9.')).toBeVisible();
  expect(local.writes.at(-1)!.expected_revision).toBe(8);
  expect(local.writes.at(-1)!.settings.retention.agent_log_days).toBe(45);
  expect(local.writes.at(-1)!.settings.model_budget).toMatchObject({
    owners: [{ owner: 'fixture-owner' }],
    default_policy: { turn_limit: 80 },
  });
  expect(local.boundary.writes).toEqual([]);
  expect(local.boundary.errors).toEqual([]);
});
