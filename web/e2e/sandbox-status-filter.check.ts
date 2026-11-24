import { expect, test, type Page } from '@playwright/test';
import { LocalSandboxFilters } from './sandbox-status-filter.fixture';
import { localTime } from './agent-startup-fixtures';
import { navigation } from './resource-lifecycle.fixture';

async function filters(page: Page) {
  const toggle = page.getByRole('button', { name: 'Filter', exact: true });
  if ((await toggle.getAttribute('aria-expanded')) !== 'true') await toggle.click();
}
async function choose(page: Page, picker: string, option: string) {
  await page.getByRole('button', { name: picker, exact: true }).click();
  await page.getByRole('menuitemradio', { name: option, exact: true }).click();
}
async function environments(page: Page) {
  await page
    .getByRole('group', { name: 'Navigation view' })
    .getByRole('button', { name: 'Environments', exact: true })
    .click();
  await filters(page);
}
function noEffects(fixture: LocalSandboxFilters) {
  expect(fixture.writes).toEqual([]);
  expect(fixture.boundary.writes).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
  expect(fixture.boundary.errors).toEqual([]);
}

for (const [width, height, entry] of [
  [1440, 900, 'pointer'],
  [1280, 800, 'collapsed keyboard'],
  [800, 600, 'keyboard'],
  [390, 844, 'pointer'],
] as const) {
  test(`LOCAL stable Tasks and Environments controls at ${width}px through ${entry}`, async ({
    page,
    baseURL,
  }, info) => {
    await page.setViewportSize({ width, height });
    const fixture = new LocalSandboxFilters();
    const { boundary } = fixture;
    boundary.agent = { ...boundary.agent, status: 'stopped' };
    const original = { ...boundary.workspace.agents[0]!, status: 'stopped' as const };
    boundary.workspace = {
      ...boundary.workspace,
      agents: [
        original,
        ...Array.from({ length: 30 }, (_, index) => ({
          ...original,
          id: `agt_mode_${index}`,
          worktree_id: `wkt_mode_${index}`,
          title: `Synthetic task ${index}`,
          task_activity_at: '2026-11-06T00:00:00Z',
        })),
      ],
    };
    await fixture.route(page, baseURL!);
    await page.route('**/auth/connections/check', (route) =>
      route.fulfill({
        json: { owner: fixture.viewer, ready: true, checked_at: localTime, services: [] },
      }),
    );
    await page.route(/\/api\/v1\/agents\/agt_mode_\d+(\/(channel-usage|provenance))?$/, (route) =>
      route.fulfill({ status: 404, json: { code: 'not_found' } }),
    );
    await page.route(/\/api\/v1\/sandboxes\/sbx_filter_0\/events(\?|$)/, (route) =>
      route.fulfill({ json: [] }),
    );
    const agentPath = `/agents/${boundary.agent.id}`;
    await page.goto(agentPath);
    await navigation(page, true);
    const mode = page.getByRole('group', { name: 'Navigation view' });
    const tasks = mode.getByRole('button', { name: 'Tasks', exact: true });
    const environments = mode.getByRole('button', { name: 'Environments', exact: true });
    const layout = async () => {
      await expect(mode.getByRole('button')).toHaveCount(2);
      await expect(tasks).toHaveText('Tasks');
      await expect(environments).toHaveText('Environments');
      const first = (await tasks.boundingBox())!;
      const second = (await environments.boundingBox())!;
      expect(first.x + first.width).toBeLessThanOrEqual(second.x);
      expect(second.x + second.width).toBeLessThanOrEqual(width);
      expect(
        await environments.evaluate((button) => button.scrollWidth <= button.clientWidth),
      ).toBe(true);
    };
    await expect(tasks).toHaveAttribute('aria-pressed', 'true');
    await layout();
    await filters(page);
    await choose(page, 'Agent status filter', 'Stopped');
    const row = page
      .locator(`[data-node-id="agent:${boundary.agent.id}"]`)
      .filter({ visible: true });
    await expect(row).toHaveAttribute('aria-selected', 'true');
    await row.focus();
    const list = page.locator('.manager-navigation__list');
    const scroll = await list.evaluate((element) => {
      element.scrollTop = 180;
      return element.scrollTop;
    });
    expect(scroll).toBeGreaterThan(0);
    await mode.screenshot({ path: info.outputPath('tasks.png') });
    if (entry === 'collapsed keyboard') {
      await navigation(page, false);
      await expect(mode).toBeHidden();
      await page.getByRole('button', { name: 'Show navigation', exact: true }).press('Enter');
      await expect(mode).toBeVisible();
      await environments.press('Enter');
    } else if (entry === 'keyboard') await environments.press('Enter');
    else await environments.click();
    await expect(environments).toBeFocused();
    await expect(environments).toHaveAttribute('aria-pressed', 'true');
    await expect(tasks).toHaveAttribute('aria-pressed', 'false');
    await layout();
    await mode.screenshot({ path: info.outputPath('environments.png') });
    await filters(page);
    await choose(page, 'Sandbox status filter', 'Provisioning');
    await choose(page, 'Navigation scope', 'Shared');
    await page.getByRole('link', { name: /provisioning fixture/ }).click();
    await expect(page).toHaveURL(`${baseURL}/sandboxes/sbx_filter_0`);
    await navigation(page, true);
    if (entry === 'pointer') await tasks.click();
    else await tasks.press('Enter');
    await expect(tasks).toHaveAttribute('aria-pressed', 'true');
    await expect(page).toHaveURL(`${baseURL}${agentPath}`);
    await expect(page.getByRole('button', { name: 'Navigation scope' })).toHaveText('Personal');
    await expect(page.getByRole('button', { name: 'Agent status filter' })).toHaveText('Stopped');
    await expect(row).toBeFocused();
    expect(await list.evaluate((element) => element.scrollTop)).toBe(scroll);
    await mode.screenshot({ path: info.outputPath('restored-tasks.png') });
    noEffects(fixture);
  });
}

test('LOCAL distinct startup states preserve filters through Tasks, reload and clear', async ({
  page,
  baseURL,
}, testInfo) => {
  const fixture = new LocalSandboxFilters();
  await fixture.route(page, baseURL!);
  await page.goto('/agent-manager');
  await filters(page);
  await choose(page, 'Agent status filter', 'Working');
  await environments(page);
  for (const status of ['Provisioning', 'Installing']) {
    await page.getByRole('button', { name: 'Sandbox status filter' }).click();
    await expect(
      page.getByRole('menuitemradio', { name: 'Provisioning', exact: true }),
    ).toHaveCount(1);
    await expect(page.getByRole('menuitemradio', { name: 'Installing', exact: true })).toHaveCount(
      1,
    );
    await expect(page.getByRole('menuitemradio', { name: 'Starting', exact: true })).toHaveCount(0);
    await page.getByRole('menuitemradio', { name: status, exact: true }).click();
    const group = page.getByRole('region', { name: `${status} sandboxes`, exact: true });
    await expect(group.locator('.resource-tree__count')).toHaveText('1');
    await expect(group.getByRole('treeitem')).toHaveCount(1);
    await expect(group.getByRole('treeitem')).toContainText(`${status.toLowerCase()} fixture`);
    await expect(group.locator('.tree-row__state')).toHaveText(status);
    await expect(
      page.getByRole('button', { name: 'Filter', exact: true }),
    ).toHaveAccessibleDescription(status);
    expect(fixture.lists.every((url) => !url.searchParams.has('status'))).toBe(true);
  }
  await page.screenshot({ path: testInfo.outputPath('installing.png') });
  await page.getByRole('button', { name: 'Tasks', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Agent status filter' })).toHaveText('Working');
  await environments(page);
  await expect(page.getByRole('button', { name: 'Sandbox status filter' })).toHaveText(
    'Installing',
  );
  await page.reload();
  await expect(page.getByRole('button', { name: 'Tasks', exact: true })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  await filters(page);
  await expect(page.getByRole('button', { name: 'Agent status filter' })).toHaveText('Working');
  await environments(page);
  await expect(page.getByRole('button', { name: 'Sandbox status filter' })).toHaveText(
    'Installing',
  );
  await page.getByRole('button', { name: 'Clear filters' }).click();
  await page.reload();
  await filters(page);
  await expect(page.getByRole('button', { name: 'Agent status filter' })).toHaveText(
    'All statuses',
  );
  await environments(page);
  await expect(page.getByRole('button', { name: 'Sandbox status filter' })).toHaveText(
    'All statuses',
  );
  noEffects(fixture);
});

for (const scope of ['Shared', 'Synthetic research (Organization)']) {
  test(`LOCAL ${scope} keeps page boundaries and isolates saved filters`, async ({
    page,
    baseURL,
  }, testInfo) => {
    const fixture = new LocalSandboxFilters();
    await fixture.route(page, baseURL!);
    await page.goto('/agent-manager');
    await environments(page);
    await choose(page, 'Sandbox status filter', 'Provisioning');
    await choose(page, 'Navigation scope', scope);
    await expect(page.getByRole('button', { name: 'Sandbox status filter' })).toHaveText(
      'All statuses',
    );
    await choose(page, 'Sandbox status filter', 'Installing');
    await expect(
      page.getByText('No Sandboxes match Installing on this page.', { exact: true }),
    ).toBeVisible();
    const scoped = () =>
      fixture.lists.filter(
        (url) => url.searchParams.has('shared') || url.searchParams.has('organization'),
      );
    expect(scoped()).toHaveLength(1);
    await page.getByRole('button', { name: 'Next Sandboxes' }).click();
    await expect(
      page.getByRole('tree', { name: 'Installing sandboxes' }).getByRole('treeitem'),
    ).toContainText('installing fixture');
    expect(scoped()).toHaveLength(2);
    expect(scoped()[1]!.searchParams.get('cursor')).toBe('sbx_filter_0');
    await choose(page, 'Sandbox status filter', 'Provisioning');
    await expect(
      page.getByText('No Sandboxes match Provisioning on this page.', { exact: true }),
    ).toBeVisible();
    expect(scoped()).toHaveLength(2);
    await page.screenshot({ path: testInfo.outputPath('page-empty.png') });
    await choose(page, 'Sandbox status filter', 'Installing');
    await choose(page, 'Navigation scope', 'Personal');
    await expect(page.getByRole('button', { name: 'Sandbox status filter' })).toHaveText(
      'Provisioning',
    );
    await choose(page, 'Navigation scope', scope);
    await expect(page.getByRole('button', { name: 'Sandbox status filter' })).toHaveText(
      'Installing',
    );
    fixture.viewer = 'second-owner';
    await page.reload();
    await environments(page);
    await choose(page, 'Navigation scope', scope);
    await expect(page.getByRole('button', { name: 'Sandbox status filter' })).toHaveText(
      'All statuses',
    );
    noEffects(fixture);
  });
}

test('LOCAL corrupt or unavailable storage keeps safe, usable filters', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalSandboxFilters();
  await fixture.route(page, baseURL!);
  await page.addInitScript(() => {
    sessionStorage.setItem(
      `cma:navigation-filters:v1:${JSON.stringify([location.origin, 'local-owner', ''])}`,
      '{"agent":"working","sandbox":"invented"}',
    );
  });
  await page.goto('/agent-manager');
  await environments(page);
  await expect(page.getByRole('button', { name: 'Sandbox status filter' })).toHaveText(
    'All statuses',
  );
  await page.evaluate(() => {
    Storage.prototype.setItem = () => {
      throw new Error('Storage unavailable');
    };
  });
  await choose(page, 'Sandbox status filter', 'Provisioning');
  await expect(
    page.getByText('Filters apply now but cannot be remembered after refresh.', { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole('tree', { name: 'Provisioning sandboxes' }).getByRole('treeitem'),
  ).toHaveCount(1);
  noEffects(fixture);
});

test('LOCAL the runtime update filter persists across Sandbox history and reload', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalSandboxFilters();
  fixture.ownedAccess = true;
  await fixture.route(page, baseURL!);
  await page.goto('/agent-manager');
  await environments(page);
  await choose(page, 'Sandbox status filter', 'Updating runtime');
  const updating = page.getByRole('tree', { name: 'Updating runtime sandboxes' });
  const selected = updating.getByRole('link', { name: /rotating fixture/ });
  await selected.click();
  await expect(page).toHaveURL(/\/sandboxes\/sbx_filter_9$/);
  await page.goBack();
  await expect(page).toHaveURL(/\/agent-manager$/);
  await expect(updating.getByRole('treeitem')).toHaveCount(1);
  await page.goForward();
  await expect(page).toHaveURL(/\/sandboxes\/sbx_filter_9$/);
  await expect(updating.getByRole('treeitem')).toHaveCount(1);
  await page.reload();
  await environments(page);
  await expect(page.getByRole('button', { name: 'Sandbox status filter' })).toHaveText(
    'Updating runtime',
  );
  await expect(updating.getByRole('treeitem')).toHaveCount(1);
  noEffects(fixture);
});

for (const scope of ['Personal', 'Shared', 'Synthetic research (Organization)']) {
  test(`LOCAL ${scope} lifecycle groups keep counts, filters, collapse and keyboard navigation`, async ({
    page,
    baseURL,
  }, testInfo) => {
    const fixture = new LocalSandboxFilters();
    fixture.paginated = false;
    const failed = fixture.sandboxes.find((row) => row.status === 'failed')!;
    fixture.sandboxes.push({
      ...failed,
      id: 'sbx_filter_failed_operation',
      name: 'failed operation fixture',
      lifecycle: {
        ...failed.lifecycle!,
        operation: {
          purpose: 'revive',
          phase: 'runtime_provisioning',
          started_at: failed.created_at,
        },
      },
    });
    for (const status of ['active', 'paused'] as const)
      fixture.sandboxes.push({
        ...failed,
        id: `sbx_filter_${status}_operation`,
        name: `${status} operation fixture`,
        status,
        lifecycle: {
          ...failed.lifecycle!,
          operation: {
            purpose: 'revive',
            phase: 'runtime_provisioning',
            started_at: failed.created_at,
          },
        },
      });
    fixture.sandboxes.push(
      {
        ...failed,
        id: 'sbx_filter_rotation_in_flight',
        name: 'runtime update before status publish',
        status: 'active',
        lifecycle: {
          ...failed.lifecycle!,
          operation: {
            purpose: 'image_rotation',
            phase: 'publish_active',
            started_at: failed.created_at,
          },
        },
      },
      {
        ...failed,
        id: 'sbx_filter_rotation_history',
        name: 'live with previous outcome',
        status: 'active',
        lifecycle: {
          ...failed.lifecycle!,
          operation: undefined,
          result: { state: 'failed', continuing: false, actions: ['observe'] },
        },
      },
    );
    await fixture.route(page, baseURL!);
    await page.goto('/agent-manager');
    await environments(page);
    if (scope !== 'Personal') await choose(page, 'Navigation scope', scope);
    for (const [label, count] of [
      ['Live', 2],
      ['Changing', 9],
      ['Paused', 1],
      ['Needs attention', 2],
    ] as const) {
      const region = page.getByRole('region', { name: `${label} sandboxes`, exact: true });
      await expect(region.locator('.resource-tree__count')).toHaveText(String(count));
      await expect(region.locator('[role="treeitem"][aria-level="1"]')).toHaveCount(count);
    }
    const live = page.getByRole('tree', { name: 'Live sandboxes', exact: true });
    await expect(live).not.toContainText('failed');
    await expect(live).not.toContainText('operation fixture');
    const liveRows = live.locator('[role="treeitem"][aria-level="1"]');
    await expect(liveRows).toHaveCount(2);
    const liveNames = await liveRows.allTextContents();
    await choose(page, 'Sandbox status filter', 'Live');
    await expect(liveRows).toHaveCount(2);
    expect(await liveRows.allTextContents()).toEqual(liveNames);
    await page.getByRole('button', { name: 'Sandbox status filter' }).click();
    const updateOption = page.getByRole('menuitemradio', { name: 'Updating runtime', exact: true });
    await expect(updateOption).toHaveCount(1);
    await updateOption.click();
    const updating = page.getByRole('region', { name: 'Updating runtime sandboxes', exact: true });
    await expect(updating.locator('.resource-tree__count')).toHaveText('2');
    await expect(updating.getByText('runtime update before status publish')).toBeVisible();
    await expect(updating.getByText('rotating fixture', { exact: true })).toBeVisible();
    await expect(updating.locator('.tree-row__state')).toHaveText([
      'Updating runtime',
      'Updating runtime',
    ]);
    await choose(page, 'Sandbox status filter', 'Updating access');
    await expect(
      page.getByRole('tree', { name: 'Updating access sandboxes' }).getByRole('treeitem'),
    ).toContainText('credential_rotating fixture');
    await expect(page.getByText('retired fixture', { exact: true })).toHaveCount(0);
    await page
      .getByText('Cleaning up 1 sandboxes', { exact: true })
      .filter({ visible: true })
      .click();
    await expect(
      page.getByRole('button', { name: 'View status for retiring fixture' }),
    ).toBeVisible();
    await choose(page, 'Sandbox status filter', 'Needs attention');
    await expect(
      page
        .getByRole('region', { name: 'Needs attention sandboxes' })
        .locator('.resource-tree__count'),
    ).toHaveText('2');
    await expect(page.getByRole('tree')).toHaveCount(1);
    await page.getByRole('button', { name: 'Clear filters' }).click();
    const changing = page.getByRole('tree', { name: 'Changing sandboxes', exact: true });
    const rows = changing.getByRole('treeitem');
    await rows.first().focus();
    await page.keyboard.press('ArrowDown');
    await expect(rows.nth(1)).toBeFocused();
    await page.getByRole('button', { name: 'Collapse Needs attention sandboxes' }).click();
    await expect(page.getByText('failed fixture', { exact: true })).toHaveCount(0);
    if (scope === 'Personal') {
      await page.reload();
      await environments(page);
    }
    await expect(
      page.getByRole('button', { name: 'Expand Needs attention sandboxes' }),
    ).toBeVisible();
    await page.getByRole('button', { name: 'Expand Needs attention sandboxes' }).click();
    await expect(page.getByText('failed fixture', { exact: true })).toBeVisible();
    await page.screenshot({ path: testInfo.outputPath('lifecycle-groups.png') });
    noEffects(fixture);
  });
}

test('LOCAL observed lifecycle transitions move a focused Sandbox exactly once without writes', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalSandboxFilters();
  const index = fixture.sandboxes.findIndex((row) => row.status === 'failed');
  fixture.sandboxes[index] = {
    ...fixture.sandboxes[index]!,
    name: 'Moving fixture',
    status: 'active',
  };
  await page.clock.install();
  await fixture.route(page, baseURL!);
  await page.goto('/agent-manager');
  await environments(page);
  const row = page
    .getByRole('treeitem')
    .filter({ has: page.getByText('Moving fixture', { exact: true }) });
  await row.focus();
  const moving = fixture.sandboxes[index]!;
  fixture.sandboxes[index] = {
    ...moving,
    lifecycle: {
      ...moving.lifecycle!,
      operation: {
        purpose: 'image_rotation',
        phase: 'publish_active',
        started_at: moving.created_at,
      },
    },
  };
  await page.clock.runFor(15_100);
  await expect(
    page.getByRole('tree', { name: 'Changing sandboxes' }).getByText('Moving fixture'),
  ).toBeVisible();
  await expect(row).toContainText('Updating runtime');
  await expect(row).toHaveCount(1);
  await expect(row).toBeFocused();
  fixture.sandboxes[index] = moving;
  await page.clock.runFor(15_100);
  await expect(
    page.getByRole('tree', { name: 'Live sandboxes' }).getByText('Moving fixture'),
  ).toBeVisible();
  await expect(row).toHaveCount(1);
  for (const [status, label] of [
    ['pausing', 'Changing'],
    ['paused', 'Paused'],
    ['reviving', 'Changing'],
    ['active', 'Live'],
    ['failed', 'Needs attention'],
  ] as const) {
    fixture.sandboxes[index] = { ...fixture.sandboxes[index]!, status };
    await page.clock.runFor(15_100);
    await expect(
      page
        .getByRole('tree', { name: `${label} sandboxes`, exact: true })
        .getByText('Moving fixture', { exact: true }),
    ).toBeVisible();
    await expect(row).toHaveCount(1);
    await expect(row).toBeFocused();
  }
  fixture.sandboxes[index] = { ...fixture.sandboxes[index]!, status: 'retiring' };
  await page.clock.runFor(15_100);
  await expect(row).toHaveCount(0);
  await page
    .getByText('Cleaning up 2 sandboxes', { exact: true })
    .filter({ visible: true })
    .click();
  await expect(page.getByRole('button', { name: 'View status for Moving fixture' })).toBeVisible();
  noEffects(fixture);
});

test('LOCAL rejects an unknown wire status with a safe error before rendering any Live Sandbox', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalSandboxFilters();
  await fixture.route(page, baseURL!);
  await page.route('**/api/v1/sandboxes?*', (route) =>
    route.fulfill({
      json: {
        items: [{ ...fixture.boundary.sandbox, name: 'Unknown fixture', status: 'future_status' }],
      },
    }),
  );
  await page.goto('/agent-manager');
  await environments(page);
  await expect(
    page.getByRole('complementary', { name: 'Application navigation' }).getByRole('alert'),
  ).toContainText('The Sandbox hierarchy could not be loaded.');
  await expect(page.getByRole('tree', { name: 'Live sandboxes' })).toHaveCount(0);
  await expect(page.getByText('Unknown fixture', { exact: true })).toHaveCount(0);
  noEffects(fixture);
});
