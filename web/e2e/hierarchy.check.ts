import { test, expect } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

test('LOCAL organization navigation and shared hierarchy enforce independent parent access', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  let revoked = false;
  const forbidden: string[] = [];
  const reads: string[] = [];
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
  const worktree = {
    access: 'shared',
    id: 'wt_shared',
    sandbox_id: sandbox.id,
    workspace_id: workspace.id,
    name: 'Review branch',
    status: 'ready',
    agent_id: 'agt_shared',
  };
  const agent = {
    id: 'agt_shared',
    worktree_id: worktree.id,
    sandbox_id: sandbox.id,
    workspace_id: workspace.id,
    status: 'ready',
    title: 'Review conversation',
    updated_at: localTime,
  };
  await page.route('**/api/v1/**', async (route) => {
    const url = new URL(route.request().url());
    const path = url.pathname;
    reads.push(path);
    const reply = (json: unknown, status = 200) => route.fulfill({ json, status });
    if (route.request().method() !== 'GET') {
      forbidden.push(path);
      return reply({ code: 'fixture_forbidden' }, 403);
    }
    if (path === '/api/v1/organizations')
      return reply({
        items: revoked ? [] : [{ id: 'org_research', display_name: 'Research', role: 'viewer' }],
      });
    if (path === '/api/v1/sandboxes' && !url.searchParams.has('organization'))
      return reply({ items: [] });
    if (revoked)
      return reply(
        { code: 'resource_not_found', detail: 'Shared resource access was removed.' },
        404,
      );
    if (path === '/api/v1/sandboxes') {
      expect(url.searchParams.get('organization')).toBe('org_research');
      return reply({ items: [sandbox] });
    }
    if (
      path === '/api/v1/resource-access/sandbox/sbx_shared' ||
      path === '/api/v1/resource-access/workspace/wks_shared'
    )
      return reply({
        capabilities: ['view_details', 'view_history'],
        revalidate_at: new Date(Date.now() + 50_000).toISOString(),
        policy_revision: 1,
      });
    if (path === '/api/v1/sandboxes/sbx_shared')
      return reply({ ...sandbox, workspaces: [workspace] });
    if (path === '/api/v1/workspaces/wks_shared')
      return reply({ ...workspace, worktrees: [worktree], agents: [agent] });
    forbidden.push(path);
    return reply({ code: 'fixture_forbidden', detail: 'No grant for this resource.' }, 403);
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/sandboxes/sbx_shared');
  const content = page.locator('.shared-hierarchy');
  await expect(content.getByRole('heading', { name: sandbox.name })).toBeVisible();
  expect(reads.some((path) => path.includes('/workspaces/'))).toBe(false);
  await page.getByRole('button', { name: 'Navigation scope', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Research', exact: true }).click();
  const tree = page.locator('.scoped-navigation');
  await tree.getByRole('button', { name: sandbox.name, exact: true }).click();
  await tree.getByRole('button', { name: workspace.name, exact: true }).click();
  await expect(tree.getByRole('link', { name: agent.title })).toHaveAttribute(
    'href',
    '/agents/agt_shared',
  );
  await content.getByRole('button', { name: workspace.name, exact: true }).click();
  await expect(content.getByRole('link', { name: agent.title })).toBeVisible();
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
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
        .toBe(true);
      await page.screenshot({
        path: info.outputPath(`hierarchy-${theme}-${width}.png`),
        fullPage: true,
        animations: 'disabled',
      });
    }
  }
  for (const name of ['Advanced', 'New workspace', 'Delete', 'Sharing', 'Pause'])
    await expect(content.getByRole('button', { name, exact: true })).toHaveCount(0);
  await content.getByRole('link', { name: 'Open Workspace details' }).click();
  await expect(content.getByRole('heading', { name: workspace.name })).toBeVisible();
  await expect(content.getByRole('link', { name: agent.title })).toHaveAttribute(
    'href',
    '/agents/agt_shared',
  );
  await page.reload();
  await expect(content.getByRole('heading', { name: workspace.name })).toBeVisible();
  revoked = true;
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await expect(content.getByRole('alert')).toContainText('Shared resource access was removed');
  for (const name of [sandbox.name, workspace.name, worktree.name, agent.title])
    await expect(page.getByText(name, { exact: true })).toHaveCount(0);
  expect(forbidden).toEqual([]);
  expect(boundary.errors).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
});
