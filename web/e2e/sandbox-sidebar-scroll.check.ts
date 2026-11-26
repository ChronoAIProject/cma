import { expect, test } from '@playwright/test';
import { environments, noEffects } from './resource-lifecycle.fixture';
import { LocalSandboxFilters } from './sandbox-status-filter.fixture';

for (const height of [450, 560]) {
  test(`LOCAL environments share one sidebar scroll region at ${height}px`, async ({
    page,
    baseURL,
  }) => {
    await page.setViewportSize({ width: 1050, height });
    const fixture = new LocalSandboxFilters();
    const { boundary } = fixture;
    boundary.sandbox = { ...boundary.sandbox, status: 'retired' };
    const template = fixture.sandboxes[0]!;
    const workspaces = Array.from({ length: 5 }, (_, index) => {
      const sandboxId = `sbx_filter_live_${index}`;
      const workspaceId = `wks_scroll_${index}`;
      const agentId = `agt_scroll_${index}`;
      const worktreeId = `wkt_scroll_${index}`;
      return {
        ...boundary.workspace,
        id: workspaceId,
        sandbox_id: sandboxId,
        agents: [
          {
            ...boundary.workspace.agents[0]!,
            id: agentId,
            title: `Task ${index + 1}`,
            sandbox_id: sandboxId,
            workspace_id: workspaceId,
            worktree_id: worktreeId,
          },
        ],
        worktrees: [
          {
            ...boundary.workspace.worktrees[0]!,
            id: worktreeId,
            sandbox_id: sandboxId,
            workspace_id: workspaceId,
            agent_id: agentId,
          },
        ],
      };
    });
    fixture.sandboxes.splice(
      0,
      fixture.sandboxes.length,
      ...workspaces.map((workspace, index) => ({
        ...template,
        id: workspace.sandbox_id,
        name: `Environment ${index + 1}`,
        status: 'active' as const,
        workspaces: [workspace],
        workspace_count: 1,
        worktree_count: 1,
        agent_count: 1,
      })),
      ...workspaces.map((_, index) => ({
        ...template,
        id: `sbx_filter_retiring_${index}`,
        name: `Retiring environment ${index + 1}`,
        status: 'retiring' as const,
      })),
    );
    await fixture.route(page, baseURL!);
    for (const workspace of workspaces)
      await page.route(`**/api/v1/workspaces/${workspace.id}`, (route) =>
        route.fulfill({ json: workspace }),
      );
    await page.route(/\/api\/v1\/sandboxes\/sbx_filter_live_\d+\/events(\?|$)/, (route) =>
      route.fulfill({ json: [] }),
    );
    await page.route(
      /\/api\/v1\/agents\/agt_scroll_\d+(\/(channel-usage|provenance))?$/,
      (route) =>
        route.request().method() === 'GET'
          ? route.fulfill({ status: 404, json: { code: 'not_found' } })
          : route.fallback(),
    );
    await page.goto('/agent-manager');
    await environments(page);

    const navigation = page.locator('.sidebar__navigation');
    const mode = navigation.getByRole('group', { name: 'Navigation view' });
    const live = navigation.getByRole('tree', { name: 'Live sandboxes' });
    await expect(live.getByRole('treeitem', { level: 1 })).toHaveCount(5);
    await expect(live.getByRole('treeitem', { level: 3 })).toHaveCount(5);
    const deletion = navigation.locator('[data-navigation-footer]');
    await deletion.locator('summary').click();
    await expect(deletion.getByRole('listitem')).toHaveCount(5);
    await navigation.evaluate((element) => (element.scrollTop = 0));
    await expect(mode).toBeInViewport();

    const last = live.getByRole('link', { name: /^Environment 5\b/ });
    await expect(last).toBeAttached();
    await expect(last).not.toBeInViewport();
    const scrollTop = () => navigation.evaluate((element) => element.scrollTop);
    await navigation.hover();
    await page.mouse.wheel(0, 100);
    await expect.poll(scrollTop).toBeGreaterThan(0);
    await last.scrollIntoViewIfNeeded();
    await expect(mode).not.toBeInViewport();
    await expect(last).toBeInViewport({ ratio: 0.9 });
    const listPosition = await scrollTop();
    const lastDeletion = deletion.getByRole('listitem').last();
    await lastDeletion.scrollIntoViewIfNeeded();
    await expect.poll(scrollTop).toBeGreaterThan(listPosition);
    await expect(lastDeletion).toBeInViewport({ ratio: 0.9 });

    await last.click();
    await expect(page).toHaveURL(`${baseURL}/sandboxes/sbx_filter_live_4`);
    await expect(live.getByRole('treeitem', { level: 1, selected: true })).toContainText(
      'Environment 5',
    );
    expect(fixture.writes).toEqual([]);
    noEffects(boundary);
  });
}
