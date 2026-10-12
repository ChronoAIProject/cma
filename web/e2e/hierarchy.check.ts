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
      path === '/api/v1/resource-access/workspace/wks_shared' ||
      path === '/api/v1/resource-access/agent/agt_shared'
    )
      return reply({
        owner: { kind: 'organization', organization_id: 'org_research' },
        capabilities: ['view_details', 'view_history'],
        revalidate_at: new Date(Date.now() + 50_000).toISOString(),
        policy_revision: 1,
      });
    if (path === '/api/v1/sandboxes/sbx_shared')
      return reply({ ...sandbox, workspaces: [workspace] });
    if (path === '/api/v1/workspaces/wks_shared')
      return reply({ ...workspace, worktrees: [worktree], agents: [agent] });
    if (path === '/api/v1/agents/agt_shared')
      return reply({
        access: 'shared',
        id: agent.id,
        title: agent.title,
        status: agent.status,
        created_at: localTime,
        updated_at: localTime,
      });
    if (path === '/api/v1/agents/agt_shared/provenance')
      return reply({ agent_id: agent.id, created_at: localTime, creation_origin: 'manual' });
    if (path === '/api/v1/agents/agt_shared/transcript')
      return reply({ agent_id: agent.id, through_seq: 0, turns: [] });
    if (path === '/api/v1/agents/agt_shared/events') return reply({ items: [] });
    if (path === '/api/v1/agui/events')
      return route.fulfill({ contentType: 'text/event-stream', body: ': keepalive\n\n' });
    if (path === '/api/v1/agents/agt_shared/channel-usage')
      return reply({
        agent_id: agent.id,
        channel_usage: { active: true, has_history: true, first_used_at: null, last_used_at: null },
      });
    forbidden.push(path);
    return reply({ code: 'fixture_forbidden', detail: 'No grant for this resource.' }, 403);
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/sandboxes/sbx_shared');
  const content = page.locator('.sandbox-page');
  await expect(content.getByRole('heading', { name: sandbox.name })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Navigation scope', exact: true })).toContainText(
    'Research',
  );
  await expect(page.getByRole('button', { name: 'New agent', exact: true })).toHaveCount(0);
  const tree = page.locator('.scoped-navigation');
  await expect(tree.getByRole('link', { name: new RegExp(agent.title) })).toHaveAttribute(
    'href',
    '/agents/agt_shared',
  );
  await expect(tree.getByRole('treeitem')).toHaveCount(3);
  await expect(tree.getByRole('link', { name: new RegExp(sandbox.name) })).toHaveAttribute(
    'aria-current',
    'page',
  );
  await content
    .getByRole('button', { name: `Show Agents in ${workspace.name}`, exact: true })
    .click();
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
  await content.getByRole('link', { name: workspace.name, exact: true }).click();
  await expect(content.getByRole('heading', { name: workspace.name })).toBeVisible();
  await expect(content.getByRole('link', { name: agent.title })).toHaveAttribute(
    'href',
    '/agents/agt_shared',
  );
  await page.reload();
  await expect(content.getByRole('heading', { name: workspace.name })).toBeVisible();
  await page.setViewportSize({ width: 1440, height: 1000 });
  const showNavigation = page.getByRole('button', { name: 'Show navigation', exact: true });
  await expect(tree).toBeVisible();
  await expect(page.getByRole('button', { name: 'Navigation scope', exact: true })).toContainText(
    'Research',
  );
  await expect(tree.getByRole('link', { name: new RegExp(workspace.name) })).toHaveAttribute(
    'aria-current',
    'page',
  );
  await tree.getByRole('link', { name: new RegExp(sandbox.name) }).click();
  await expect(content.getByRole('heading', { name: sandbox.name })).toBeVisible();
  await tree.getByRole('link', { name: new RegExp(workspace.name) }).click();
  await expect(content.getByRole('heading', { name: workspace.name })).toBeVisible();
  await tree.getByRole('link', { name: new RegExp(agent.title) }).click();
  await expect(
    page.locator('.agent-header').getByRole('heading', { name: agent.title }),
  ).toBeVisible();
  await expect(tree.getByRole('link', { name: new RegExp(agent.title) })).toHaveAttribute(
    'aria-current',
    'page',
  );
  await page.reload();
  await expect(page.getByRole('button', { name: 'Navigation scope', exact: true })).toContainText(
    'Research',
  );
  await expect(tree.getByRole('link', { name: new RegExp(agent.title) })).toHaveAttribute(
    'aria-current',
    'page',
  );
  await page.screenshot({
    path: info.outputPath('organization-agent-desktop.png'),
    fullPage: true,
    animations: 'disabled',
  });
  await page.setViewportSize({ width: 390, height: 1000 });
  await expect(showNavigation).toBeVisible();
  await showNavigation.click();
  await tree.getByRole('link', { name: new RegExp(workspace.name) }).click();
  await expect(content.getByRole('heading', { name: workspace.name })).toBeVisible();
  await expect(showNavigation).toBeVisible();
  await page.screenshot({
    path: info.outputPath('organization-workspace-mobile.png'),
    fullPage: true,
    animations: 'disabled',
  });
  await page.reload();
  await expect(content.getByRole('heading', { name: workspace.name })).toBeVisible();
  await showNavigation.click();
  await expect(page.getByRole('button', { name: 'Navigation scope', exact: true })).toContainText(
    'Research',
  );
  await tree.getByRole('link', { name: new RegExp(agent.title) }).click();
  await expect(
    page.locator('.agent-header').getByRole('heading', { name: agent.title }),
  ).toBeVisible();
  await page.screenshot({
    path: info.outputPath('organization-agent-mobile.png'),
    fullPage: true,
    animations: 'disabled',
  });
  revoked = true;
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await expect(page.getByRole('heading', { name: 'Agent unavailable' })).toBeVisible();
  for (const name of [sandbox.name, workspace.name, worktree.name, agent.title])
    await expect(page.getByText(name, { exact: true })).toHaveCount(0);
  expect(forbidden).toEqual([]);
  expect(boundary.errors).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
});
