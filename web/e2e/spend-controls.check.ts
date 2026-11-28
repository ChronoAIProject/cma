import { test, expect, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { testBudget } from '../src/operator/spendTestFixtures';
import { ownerAmounts } from '../src/operator/spendBudget';
import type { PlatformSettings } from '@cma/client';

async function fixture(
  page: Page,
  baseURL: string,
  availability: { value?: boolean; request?: boolean; budget?: unknown } = { value: true },
) {
  let available = availability.value;
  let requestAvailable = availability.request;
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
    model_budget: Object.hasOwn(availability, 'budget')
      ? availability.budget
      : structuredClone(testBudget),
    future: { retained: true },
  };
  const writes: { expected_revision: number; settings: PlatformSettings }[] = [];
  let failure = 0;
  await page.route('**/api/v1/admin/settings', async (route) => {
    if (route.request().method() === 'PUT') {
      const body = route.request().postDataJSON() as (typeof writes)[number];
      writes.push(body);
      // REST restores omitted policy before core checks compatible-reader gates.
      const effective = {
        ...body.settings,
        model_budget:
          body.settings.model_budget === undefined
            ? settings.model_budget
            : body.settings.model_budget,
      };
      const budget = effective.model_budget as
        | {
            enabled?: boolean;
            platform?: unknown;
            default_policy?: unknown;
            owners?: unknown[];
            request_models?: unknown[];
          }
        | undefined;
      const requests = (budget?.request_models?.length ?? 0) > 0;
      const nonempty =
        !!budget &&
        (budget.enabled ||
          budget.platform != null ||
          budget.default_policy != null ||
          (budget.owners?.length ?? 0) > 0 ||
          requests);
      const gateClosed =
        (available !== true && nonempty) || (requestAvailable !== true && requests);
      if (failure || gateClosed)
        return route.fulfill({
          status: gateClosed ? 503 : failure,
          json: {
            status: gateClosed ? 503 : failure,
            title: 'Settings refused',
            code: gateClosed
              ? 'model_budget_writes_unavailable'
              : failure === 409
                ? 'settings_conflict'
                : 'invalid_request',
          },
        });
      expect(body.expected_revision).toBe(revision);
      expect(body.settings.future).toEqual(settings.future);
      settings = effective;
      revision += 1;
    } else expect(route.request().method()).toBe('GET');
    await route.fulfill({
      json: {
        revision,
        settings,
        model_budget_writes_available: available,
        model_request_budget_writes_available: requestAvailable,
      },
    });
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
    setAvailable: (value: boolean) => {
      available = value;
    },
    setRequestAvailable: (value: boolean) => {
      requestAvailable = value;
    },
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

for (const available of [false, undefined]) {
  test(`LOCAL spend writes stay disabled when availability is ${available}`, async ({
    page,
    baseURL,
  }) => {
    const local = await fixture(page, baseURL!, { value: available });
    await page.goto('/operations?tab=settings');
    await expect(page.getByText(/Saving spend controls is not available yet/)).toBeVisible();
    await expect(page.getByLabel('Enable spend controls')).toBeDisabled();
    await expect(
      page.getByLabel('Per-turn cap (accounting units)', { exact: true }),
    ).toBeDisabled();
    await page.getByLabel('Agent log retention (days)').fill('45');
    await expect(page.getByRole('button', { name: 'Save settings' })).toBeDisabled();
    await expect(
      page.getByText(/Saving settings is not available yet for this spend policy/),
    ).toBeVisible();
    expect(local.writes).toHaveLength(0);
    await expect(page.getByLabel('Per-turn cap (accounting units)', { exact: true })).toHaveValue(
      String(testBudget.default_policy!.turn_limit),
    );
    expect(local.boundary.errors).toEqual([]);
  });

  test(`LOCAL an empty disabled policy allows unrelated saves with availability ${available}`, async ({
    page,
    baseURL,
  }) => {
    const budget = { enabled: false, owners: [] };
    const local = await fixture(page, baseURL!, { value: available, budget });
    await page.goto('/operations?tab=settings');
    await expect(page.getByLabel('Enable spend controls')).toBeDisabled();
    await page.getByLabel('Agent log retention (days)').fill('45');
    await page.getByRole('button', { name: 'Save settings' }).click();
    await expect(page.getByText('Settings saved at revision 5.')).toBeVisible();
    expect(local.writes[0]!.settings.model_budget).toEqual(budget);
    expect(local.boundary.errors).toEqual([]);
  });

  test(`LOCAL retained request models block all saves with request availability ${available}`, async ({
    page,
    baseURL,
  }) => {
    const local = await fixture(page, baseURL!, {
      value: true,
      request: available,
      budget: {
        ...testBudget,
        request_models: [{ model: 'fixture-model', input_overhead_tokens: 1 }],
      },
    });
    await page.goto('/operations?tab=settings');
    await expect(
      page.getByLabel('Per-turn cap (accounting units)', { exact: true }),
    ).toBeDisabled();
    await page.getByLabel('Agent log retention (days)').fill('45');
    await expect(page.getByRole('button', { name: 'Save settings' })).toBeDisabled();
    expect(local.writes).toHaveLength(0);
    expect(local.boundary.errors).toEqual([]);
  });
}

for (const gate of ['foundation', 'request']) {
  test(`LOCAL a closed ${gate} gate retains the draft for explicit available-revision review`, async ({
    page,
    baseURL,
  }) => {
    const models = [{ model: 'fixture-model', input_overhead_tokens: 1 }];
    const local = await fixture(page, baseURL!, {
      value: true,
      request: true,
      budget: { ...testBudget, request_models: models },
    });
    const setGate = gate === 'foundation' ? local.setAvailable : local.setRequestAvailable;
    await page.goto('/operations?tab=settings');
    const cap = page.getByLabel('Per-turn cap (accounting units)', { exact: true });
    await cap.fill('80');
    setGate(false);
    await page.getByRole('button', { name: 'Save settings' }).click();
    await expect(page.getByRole('alert')).toContainText(
      'Saving spend controls is not available yet',
    );
    await expect(page.getByText(/The current revision is 4/)).toBeVisible();
    await expect(cap).toHaveValue('80');
    await expect(cap).toBeDisabled();
    await page.getByRole('button', { name: 'Review retained draft' }).click();
    await expect(page.getByRole('button', { name: 'Save settings' })).toBeDisabled();
    expect(local.writes).toHaveLength(1);
    setGate(true);
    await page.getByRole('button', { name: 'Refresh settings availability' }).click();
    await expect(page.getByText(/The current revision is 4/)).toBeVisible();
    await page.getByRole('button', { name: 'Review retained draft' }).click();
    await expect(cap).toBeEnabled();
    await page.getByRole('button', { name: 'Save settings' }).click();
    await expect(page.getByText('Settings saved at revision 5.')).toBeVisible();
    expect(local.writes.at(-1)!.settings.model_budget).toMatchObject({
      default_policy: { turn_limit: 80 },
      request_models: models,
    });
    expect(local.writes.at(-1)!).not.toHaveProperty('model_budget_writes_available');
    expect(local.boundary.errors).toEqual([]);
  });
}

async function addOwner(page: Page) {
  await page.getByRole('button', { name: 'Add owner override' }).click();
  await page.getByLabel('Owner override 1: owner ID').fill('fixture-isolated-owner');
  for (const [field, label] of Object.entries(ownerAmounts))
    await page
      .getByLabel(`Owner override 1: ${label}`, { exact: true })
      .fill(String(testBudget.default_policy![field as keyof typeof ownerAmounts]));
  await page
    .getByLabel('Owner override 1: Day-boundary timezone (IANA)')
    .fill(testBudget.default_policy!.day_timezone);
}

test('LOCAL isolated owner and request-model editing saves explicit policy and validates drafts', async ({
  page,
  baseURL,
}, testInfo) => {
  const local = await fixture(page, baseURL!, { value: true, request: true });
  await page.goto('/operations?tab=settings');
  await addOwner(page);
  await page.getByRole('button', { name: 'Remove default owner policy' }).click();
  await page.getByRole('button', { name: 'Add request model' }).click();
  const model = page.getByLabel('Request model 1: model name');
  const overhead = page.getByLabel('Request model 1: input overhead (tokens)');
  await expect(model).toHaveValue('');
  await expect(overhead).toHaveValue('');
  await model.fill('fixture-model');
  await overhead.fill('1.5');
  await page.getByRole('button', { name: 'Save settings' }).click();
  await expect(overhead).toHaveAttribute('aria-invalid', 'true');
  expect(local.writes).toHaveLength(0);
  await overhead.fill('1');
  await page.getByRole('button', { name: 'Save settings' }).click();
  await expect(page.getByText('Settings saved at revision 5.')).toBeVisible();
  expect(local.writes[0]!.expected_revision).toBe(4);
  expect(local.writes[0]!.settings.model_budget).toEqual({
    enabled: false,
    platform: testBudget.platform,
    owners: [{ owner: 'fixture-isolated-owner', policy: testBudget.default_policy }],
    request_models: [{ model: 'fixture-model', input_overhead_tokens: 1 }],
  });
  await expect(page.getByLabel('Enable spend controls')).not.toBeChecked();
  await page.getByRole('group', { name: 'Owner override 1', exact: true }).screenshot({
    path: testInfo.outputPath('owner-policy.png'),
  });
  await page.getByRole('group', { name: 'Request model 1', exact: true }).screenshot({
    path: testInfo.outputPath('request-model.png'),
  });
  await page.getByRole('button', { name: 'Remove owner override 1' }).click();
  await page.getByRole('button', { name: 'Remove request model 1' }).click();
  await page.getByRole('button', { name: 'Save settings' }).click();
  await expect(page.getByText('Settings saved at revision 6.')).toBeVisible();
  expect(local.writes[1]!.settings.model_budget).toEqual({
    enabled: false,
    platform: testBudget.platform,
    owners: [],
    request_models: [],
  });
  expect(local.boundary.writes).toEqual([]);
  expect(local.boundary.errors).toEqual([]);
});

test('LOCAL concurrent owner-list edits stay visible without overwriting the server list', async ({
  page,
  baseURL,
}) => {
  const local = await fixture(page, baseURL!, { value: true, request: true });
  await page.goto('/operations?tab=settings');
  await addOwner(page);
  local.advance();
  local.setFailure(409);
  await page.getByRole('button', { name: 'Save settings' }).click();
  await expect(page.getByText(/changed both here and on the server/)).toBeVisible();
  await expect(page.getByLabel('Owner override 1: owner ID')).toHaveValue('fixture-isolated-owner');
  await expect(page.getByRole('button', { name: 'Review retained draft' })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Save settings' })).toBeDisabled();
  expect(local.writes).toHaveLength(1);
  await page.getByRole('button', { name: 'Load current values' }).click();
  await expect(page.getByLabel('Owner override 1: owner ID')).toHaveValue('fixture-owner');
  await expect(page.getByRole('button', { name: 'Save settings' })).toBeEnabled();
  expect(local.writes).toHaveLength(1);
  expect(local.boundary.errors).toEqual([]);
});

for (const available of [false, undefined]) {
  test(`LOCAL request-model editor waits for explicit gate at ${available} without blocking owner edits`, async ({
    page,
    baseURL,
  }) => {
    const local = await fixture(page, baseURL!, { value: true, request: available });
    await page.goto('/operations?tab=settings');
    await expect(page.getByRole('button', { name: 'Add request model' })).toBeDisabled();
    await expect(page.getByText(/Saving request models is not available yet/)).toBeVisible();
    await addOwner(page);
    await page.getByRole('button', { name: 'Save settings' }).click();
    await expect(page.getByText('Settings saved at revision 5.')).toBeVisible();
    expect(local.writes).toHaveLength(1);
    local.setRequestAvailable(true);
    await page.getByRole('button', { name: 'Refresh settings availability' }).click();
    await expect(page.getByRole('button', { name: 'Add request model' })).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Review retained draft' })).toHaveCount(0);
    await expect(page.getByLabel('Owner override 1: owner ID')).toHaveValue(
      'fixture-isolated-owner',
    );
    expect(local.writes).toHaveLength(1);
    expect(local.boundary.errors).toEqual([]);
  });
}
