import { test, expect } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

test('LOCAL shared aggregate preserves personal work and supports filtered pages and safe exports', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  boundary.sandbox = { ...boundary.sandbox, owner: 'local-owner' };
  await boundary.route(page, baseURL!);
  await page.route('**/auth/connections/check', (route) =>
    route.fulfill({
      json: { owner: 'local-owner', ready: true, checked_at: localTime, services: [] },
    }),
  );
  let revoked = false;
  const reads: URL[] = [];
  const rows = [
    {
      agent_id: 'agt_shared_a',
      title: 'Shared investigation',
      status: 'ready',
      created_at: localTime,
      updated_at: localTime,
      creation_origin: 'manual',
      channel_usage: { active: true, has_history: true, first_used_at: null, last_used_at: null },
    },
    {
      agent_id: 'agt_shared_b',
      title: 'Shared delivery',
      status: 'stopped',
      created_at: localTime,
      updated_at: localTime,
      creation_origin: 'trigger',
      channel_usage: {
        active: false,
        has_history: true,
        first_used_at: localTime,
        last_used_at: localTime,
      },
    },
  ] as const;
  const sandbox = {
    access: 'shared',
    id: 'sbx_shared',
    name: 'Research Sandbox',
    status: 'active',
    created_at: localTime,
    updated_at: localTime,
    last_activity_at: localTime,
    workspace_count: 1,
    worktree_count: 1,
    agent_count: 1,
  };
  const workspace = {
    access: 'shared',
    id: 'wks_shared',
    sandbox_id: sandbox.id,
    name: 'Review Workspace',
    status: 'ready',
    created_at: localTime,
    worktree_count: 1,
    agent_count: 1,
  };
  const branchReads: string[] = [];
  await page.route('**/api/v1/resource-access/agent/*', (route) => {
    const id = new URL(route.request().url()).pathname.split('/').at(-1);
    if (id === 'agt_contract') return route.fallback();
    return route.fulfill(
      revoked
        ? {
            status: 404,
            json: { code: 'resource_not_found', detail: 'Shared access removed' },
          }
        : {
            json: {
              capabilities: id === 'agt_shared_b' ? ['view_details', 'interact'] : ['view_details'],
              owner:
                id === 'agt_shared_a'
                  ? { kind: 'person', is_current_person: false }
                  : { kind: 'organization', organization_id: 'org_research' },
              revalidate_at: null,
              policy_revision: 1,
            },
          },
    );
  });
  await page.route('**/api/v1/agents/agt_branch/channel-usage', (route) =>
    route.fulfill(
      revoked
        ? {
            status: 404,
            json: { code: 'resource_not_found', detail: 'Shared access removed' },
          }
        : { json: { agent_id: 'agt_branch', channel_usage: rows[0].channel_usage } },
    ),
  );
  await page.route('**/api/v1/sandboxes?*', async (route) => {
    const url = new URL(route.request().url());
    if (!url.searchParams.has('shared')) return route.fallback();
    expect(url.searchParams.get('shared')).toBe('true');
    expect(url.searchParams.has('organization')).toBe(false);
    if (revoked)
      return route.fulfill({
        status: 404,
        json: { code: 'resource_not_found', detail: 'Shared access removed' },
      });
    return route.fulfill({
      json: url.searchParams.has('cursor')
        ? { items: [sandbox] }
        : { items: [], next: 'sbx_previous' },
    });
  });
  for (const path of [
    'resource-access/sandbox/sbx_shared',
    'resource-access/workspace/wks_shared',
    'sandboxes/sbx_shared',
    'workspaces/wks_shared',
  ]) {
    await page.route(`**/api/v1/${path}`, (route) => {
      branchReads.push(path);
      if (revoked)
        return route.fulfill({
          status: 404,
          json: { code: 'resource_not_found', detail: 'Shared access removed' },
        });
      if (path.startsWith('resource-access/'))
        return route.fulfill({
          json: {
            capabilities: ['view_details'],
            revalidate_at: null,
            policy_revision: 1,
            owner: { kind: 'organization', organization_id: 'org_research' },
          },
        });
      if (path.startsWith('sandboxes/'))
        return route.fulfill({ json: { ...sandbox, workspaces: [workspace] } });
      return route.fulfill({
        json: {
          ...workspace,
          worktrees: [
            {
              access: 'shared',
              id: 'wt_shared',
              workspace_id: workspace.id,
              sandbox_id: sandbox.id,
              name: 'Review branch',
              status: 'ready',
              agent_id: 'agt_branch',
            },
          ],
          agents: [
            {
              id: 'agt_branch',
              workspace_id: workspace.id,
              sandbox_id: sandbox.id,
              worktree_id: 'wt_shared',
              title: 'Authorized branch conversation',
              status: 'ready',
              updated_at: localTime,
            },
          ],
        },
      });
    });
  }
  await page.route('**/api/v1/organizations', (route) =>
    route.fulfill({
      json: {
        items: revoked ? [] : [{ id: 'org_research', display_name: 'Research', role: 'admin' }],
      },
    }),
  );
  await page.route('**/api/v1/agents/provenance?*', (route) => {
    const url = new URL(route.request().url());
    reads.push(url);
    expect(route.request().method()).toBe('GET');
    expect(url.searchParams.get('shared')).toBe('true');
    expect(url.searchParams.has('organization')).toBe(false);
    if (revoked)
      return route.fulfill({
        status: 404,
        json: { code: 'resource_not_found', detail: 'Shared access removed' },
      });
    if (url.searchParams.has('creation_origin')) {
      return route.fulfill({
        json: url.searchParams.has('cursor')
          ? { items: [rows[1]] }
          : { items: [], next: rows[0].agent_id },
      });
    }
    return route.fulfill({ json: { items: rows } });
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/agents/agt_contract');
  const composer = page.getByRole('textbox', { name: 'Message' });
  await composer.fill('Keep this personal draft');
  const composerNode = await composer.elementHandle();
  await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
  // Scope sits behind the navigation Filter button.
  await page.getByRole('button', { name: /^Filter/ }).click();
  await page.getByRole('button', { name: 'Navigation scope', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Shared with me', exact: true }).click();
  const navigation = page.locator('.scoped-navigation');
  await expect(navigation.getByRole('link', { name: rows[0].title })).toBeVisible();
  await expect(navigation.getByText('Owner: Personal owner · Your access: View only')).toHaveCount(
    1,
  );
  await expect(navigation.getByText(/No shared Sandboxes on this page/)).toBeVisible();
  expect(branchReads).toEqual([]);
  await navigation.getByRole('button', { name: 'Next Sandboxes', exact: true }).click();
  await expect(navigation.getByRole('link', { name: /^Research Sandbox/ })).toHaveAttribute(
    'href',
    '/sandboxes/sbx_shared',
  );
  await expect(navigation.getByRole('link', { name: /^Review Workspace/ })).toHaveAttribute(
    'href',
    '/workspaces/wks_shared',
  );
  await expect(
    navigation.getByRole('link', { name: 'Authorized branch conversation' }),
  ).toHaveAttribute('href', '/agents/agt_branch');
  await expect(navigation.getByRole('link', { name: rows[0].title })).toBeVisible();
  await expect(navigation.getByRole('button', { name: 'Next shared Agents' })).toBeDisabled();
  await expect(
    navigation.getByText('Owner: Research · Your access: View only').first(),
  ).toHaveCount(1);
  for (const theme of ['light', 'dark']) {
    await page.evaluate((value) => {
      document.body.toggleAttribute('data-ds-dark-theme', value === 'dark');
      document.documentElement.style.colorScheme = value;
    }, theme);
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      await expect
        .poll(() =>
          page
            .locator('.frame__center')
            .evaluate((element) => element.getBoundingClientRect().right <= innerWidth + 1),
        )
        .toBe(true);
      if (width < 1024) {
        const show = page.getByRole('button', { name: 'Show navigation', exact: true });
        if (await show.isVisible()) {
          await show.click();
          await navigation.getByRole('button', { name: 'Next Sandboxes', exact: true }).click();
        }
      }
      await expect(
        navigation.getByRole('link', { name: 'Authorized branch conversation' }),
      ).toBeVisible();
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
        .toBe(true);
      await expect
        .poll(() =>
          navigation.evaluate((element) =>
            [...element.querySelectorAll('button, a')].every(
              (item) => item.getBoundingClientRect().right <= innerWidth + 1,
            ),
          ),
        )
        .toBe(true);
      await page.screenshot({
        path: info.outputPath(`shared-tree-${theme}-${width}.png`),
        fullPage: true,
        animations: 'disabled',
      });
    }
  }
  await page.setViewportSize({ width: 1440, height: 1000 });
  revoked = true;
  await navigation.getByRole('button', { name: 'Refresh shared Agents' }).click();
  await expect(navigation.getByRole('alert')).toContainText('Shared access removed');
  await expect(navigation.getByText(rows[0].title)).toHaveCount(0);
  await navigation.getByRole('button', { name: 'Refresh shared Sandboxes' }).click();
  for (const name of [sandbox.name, workspace.name, 'Authorized branch conversation'])
    await expect(navigation.getByText(name, { exact: true })).toHaveCount(0);
  expect(await composer.evaluate((element, previous) => element === previous, composerNode)).toBe(
    true,
  );
  await expect(composer).toHaveValue('Keep this personal draft');
  await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
  revoked = false;
  await navigation.getByRole('link', { name: 'All shared Agents' }).click();
  await expect(page).toHaveURL(/\/shared\/agents$/);
  const inventory = page.locator('.agent-inventory');
  await expect(inventory.getByRole('link', { name: rows[0].title })).toBeVisible();
  await page.reload();
  await expect(inventory.getByRole('button', { name: 'Resource scope' })).toContainText(
    'Shared with me',
  );
  await inventory.getByRole('button', { name: 'Creation origin' }).click();
  await page.getByRole('menuitemradio', { name: 'Created by Trigger', exact: true }).click();
  await inventory.getByRole('button', { name: 'Channel activity' }).click();
  await page
    .getByRole('menuitemradio', { name: 'Previously linked, now detached', exact: true })
    .click();
  await expect(
    inventory.getByText('No matches on this page. Continue to check the next page.'),
  ).toBeVisible();
  await inventory.getByRole('button', { name: 'Next page' }).click();
  await expect(inventory.getByRole('link', { name: rows[1].title })).toHaveAttribute(
    'href',
    '/agents/agt_shared_b',
  );
  await expect(
    inventory.getByText('Previously linked to a channel', { exact: true }),
  ).toBeVisible();
  await expect(inventory.getByText('Owner: Research · Your access: Can interact')).toBeVisible();
  expect(reads.at(-1)?.searchParams.get('channel_active')).toBe('false');
  expect(reads.at(-1)?.searchParams.get('channel_history')).toBe('true');
  const download = page.waitForEvent('download');
  const beforeExport = reads.length;
  await inventory.getByRole('button', { name: 'Export this page' }).click();
  const file = await download;
  expect(reads.length).toBeGreaterThan(beforeExport);
  const stream = await file.createReadStream();
  const chunks = [];
  for await (const chunk of stream) chunks.push(chunk);
  const exported = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  expect(exported).toEqual({ agents: [rows[1]] });
  for (const theme of ['light', 'dark']) {
    await page.evaluate((value) => {
      document.body.toggleAttribute('data-ds-dark-theme', value === 'dark');
      document.documentElement.style.colorScheme = value;
    }, theme);
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
        .toBe(true);
      await expect
        .poll(() =>
          page
            .locator('.frame__center')
            .evaluate((element) => element.getBoundingClientRect().right <= innerWidth + 1),
        )
        .toBe(true);
      await expect
        .poll(() =>
          inventory.evaluate((element) =>
            [...element.querySelectorAll('button, a, time')].every(
              (item) => item.getBoundingClientRect().right <= innerWidth + 1,
            ),
          ),
        )
        .toBe(true);
      await page.screenshot({
        path: info.outputPath(`shared-inventory-${theme}-${width}.png`),
        fullPage: true,
        animations: 'disabled',
      });
    }
  }
  revoked = true;
  await inventory.getByRole('button', { name: 'Refresh', exact: true }).click();
  await expect(inventory.getByRole('alert')).toContainText('Shared access removed');
  await expect(inventory.getByRole('link', { name: rows[1].title })).toHaveCount(0);
  expect(boundary.unexpected).toEqual([]);
});

test('LOCAL shared task navigation uses the detail title and keeps long metadata readable', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  let title = 'Shared investigation from the authorized detail';
  const longOwner =
    'International research and applied engineering organization with a deliberately long label';
  const row = {
    agent_id: 'agt_contract',
    title: 'New Agent',
    status: 'ready',
    task_status: 'completed',
    creation_origin: 'manual',
    created_at: localTime,
    updated_at: localTime,
  };
  await page.route('**/api/v1/agents/agt_contract', (route) =>
    route.fulfill({
      json: {
        id: row.agent_id,
        access: 'shared',
        title,
        status: row.status,
        created_at: localTime,
        updated_at: localTime,
      },
    }),
  );
  await page.route('**/api/v1/resource-access/agent/agt_contract', (route) =>
    route.fulfill({
      json: {
        owner: { kind: 'organization', organization_id: 'org_research' },
        capabilities: ['view_details', 'view_history', 'view_files'],
        revalidate_at: null,
        policy_revision: 1,
      },
    }),
  );
  await page.route('**/api/v1/organizations', (route) =>
    route.fulfill({
      json: { items: [{ id: 'org_research', display_name: longOwner, role: 'viewer' }] },
    }),
  );
  await page.route('**/api/v1/agents/provenance?*', (route) =>
    route.fulfill({ json: { items: [row] } }),
  );
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/agents/agt_contract');
  await expect(page.getByRole('heading', { name: title, exact: true })).toBeVisible();
  await page.getByRole('button', { name: /^Filter/ }).click();
  await page.getByRole('button', { name: 'Navigation scope', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Shared', exact: true }).click();
  await page.getByRole('button', { name: 'Tasks', exact: true }).click();
  const navigation = page.getByRole('tree', { name: 'Shared Agents' });
  const item = navigation.getByRole('treeitem', { name: new RegExp(title) });
  await expect(item).toBeVisible();
  await expect(item.locator('.resource-access-summary')).toHaveAttribute(
    'title',
    new RegExp(longOwner),
  );
  for (const width of [1440, 375]) {
    await page.setViewportSize({ width, height: 812 });
    if (width === 375) {
      const show = page.getByRole('button', { name: 'Show navigation', exact: true });
      await expect(show).toBeVisible();
      await show.click();
    }
    await expect(item).toBeVisible();
    const facts = item.locator('.tree-row__meta > :not(.sr-only)');
    expect(
      await facts.evaluateAll((items) =>
        items.every((el) => {
          const style = getComputedStyle(el);
          return (
            style.whiteSpace === 'nowrap' &&
            el.getBoundingClientRect().height <= Number.parseFloat(style.lineHeight) + 1
          );
        }),
      ),
    ).toBe(true);
  }
  await page.setViewportSize({ width: 1440, height: 1000 });
  title = 'Renamed shared investigation';
  await page.getByRole('button', { name: 'Refresh Agents', exact: true }).click();
  await expect(navigation.getByRole('treeitem', { name: new RegExp(title) })).toBeVisible();
});
