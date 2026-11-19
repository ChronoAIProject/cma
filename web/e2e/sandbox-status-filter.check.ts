import { expect, test, type Page } from '@playwright/test';
import { LocalSandboxFilters } from './sandbox-status-filter.fixture';

async function filters(page: Page) {
  const toggle = page.getByRole('button', { name: 'Filter', exact: true });
  if ((await toggle.getAttribute('aria-expanded')) !== 'true') await toggle.click();
}
async function choose(page: Page, picker: string, option: string) {
  await page.getByRole('button', { name: picker, exact: true }).click();
  await page.getByRole('menuitemradio', { name: option, exact: true }).click();
}
async function environments(page: Page) {
  await filters(page);
  await page.getByRole('button', { name: 'Manage environments', exact: true }).click();
  await filters(page);
}
function noEffects(fixture: LocalSandboxFilters) {
  expect(fixture.writes).toEqual([]);
  expect(fixture.boundary.writes).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
  expect(fixture.boundary.errors).toEqual([]);
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
  await page.getByRole('button', { name: 'Back to tasks' }).click();
  await expect(page.getByRole('button', { name: 'Agent status filter' })).toHaveText('Working');
  await environments(page);
  await expect(page.getByRole('button', { name: 'Sandbox status filter' })).toHaveText(
    'Installing',
  );
  await page.reload();
  await expect(page.getByRole('button', { name: 'Back to tasks' })).toHaveCount(0);
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
