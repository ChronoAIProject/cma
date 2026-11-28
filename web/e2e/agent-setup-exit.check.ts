import { expect, test } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

for (const scenario of [
  { status: 'provisioning', entry: 'panel', width: 1440, uncertain: true },
  { status: 'installing', entry: 'header', width: 390, uncertain: false },
  { status: 'installing', entry: 'list', width: 1440, uncertain: false },
  { status: 'provisioning', entry: 'header', width: 1440, uncertain: false, activeOnArrival: true },
] as const) {
  test(`LOCAL ${scenario.status} Agent exits through ${scenario.entry} at ${scenario.width}px`, async ({
    page,
    baseURL,
  }) => {
    const boundary = new LocalStartup();
    boundary.agent = {
      ...boundary.agent,
      status: 'starting',
      ready_at: undefined,
      title: 'Preparing Agent',
      sandbox: { ...boundary.agent.sandbox, status: scenario.status },
    };
    boundary.workspace = {
      ...boundary.workspace,
      agent_count: 2,
      worktree_count: 2,
      worktrees: [
        ...boundary.workspace.worktrees,
        { ...boundary.workspace.worktrees[0]!, id: 'wkt_sibling', agent_id: 'agt_sibling' },
      ],
      agents: [
        { ...boundary.workspace.agents[0]!, status: 'starting', title: 'Preparing Agent' },
        {
          ...boundary.workspace.agents[0]!,
          id: 'agt_sibling',
          worktree_id: 'wkt_sibling',
          status: 'starting',
          title: 'Sibling Agent',
        },
      ],
    };
    boundary.sandbox = {
      ...boundary.sandbox,
      status: scenario.status,
      agent_count: 2,
      worktree_count: 2,
      workspaces: [{ ...boundary.sandbox.workspaces[0]!, agent_count: 2, worktree_count: 2 }],
    };
    await page.setViewportSize({ width: scenario.width, height: 900 });
    await boundary.route(page, baseURL!);
    await page.route('**/api/v1/agents/agt_sibling', (route) => {
      if (route.request().method() !== 'GET') return route.fallback();
      return route.fulfill({
        json: {
          ...boundary.agent,
          id: 'agt_sibling',
          title: 'Sibling Agent',
          worktree: { ...boundary.agent.worktree, id: 'wkt_sibling' },
          sandbox: { ...boundary.agent.sandbox, status: boundary.sandbox.status },
        },
      });
    });
    await page.route('**/api/v1/agents/agt_sibling/channel-usage', (route) =>
      route.fulfill({
        json: {
          agent_id: 'agt_sibling',
          channel_usage: {
            active: false,
            has_history: false,
            first_used_at: null,
            last_used_at: null,
          },
        },
      }),
    );
    await page.route('**/api/v1/agents/agt_sibling/provenance', (route) =>
      route.fulfill({
        json: {
          agent_id: 'agt_sibling',
          created_at: boundary.agent.created_at,
          creation_origin: 'legacy_unknown',
        },
      }),
    );
    const writes: string[] = [];
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (path.startsWith('/api/') && !['GET', 'HEAD'].includes(request.method()))
        writes.push(`${request.method()} ${path}`);
    });
    const deletes: { key?: string; body: string | null }[] = [];
    await page.route(`**/api/v1/sandboxes/${boundary.sandbox.id}`, (route) => {
      const request = route.request();
      if (
        request.method() === 'GET' &&
        'activeOnArrival' in scenario &&
        new URL(page.url()).pathname === `/sandboxes/${boundary.sandbox.id}` &&
        boundary.sandbox.status === 'provisioning'
      )
        boundary.sandbox = {
          ...boundary.sandbox,
          status: 'active',
          revision: boundary.sandbox.revision + 1,
        };
      if (request.method() !== 'DELETE') return route.fallback();
      deletes.push({ key: request.headers()['idempotency-key'], body: request.postData() });
      if (scenario.uncertain && deletes.length === 1)
        return route.fulfill({
          status: 503,
          json: { code: 'unavailable', detail: 'Delete response unavailable.' },
        });
      boundary.sandbox = { ...boundary.sandbox, status: 'retiring' };
      return route.fulfill({ status: 202, json: boundary.sandbox });
    });
    await page.goto(`/agents/${boundary.agent.id}`);
    const panel = page.getByRole('region', { name: 'Cancel Agent setup' });
    await expect(panel).toContainText('This removes the environment and all Agents inside it.');
    if (scenario.entry === 'panel') {
      await panel.getByRole('button', { name: 'Cancel setup…', exact: true }).click();
    } else {
      if (scenario.entry === 'header') {
        await page.getByRole('button', { name: 'More Agent actions', exact: true }).click();
        await expect(page.getByRole('menuitem', { name: /^Stop agent/ })).toHaveAttribute(
          'aria-disabled',
          'true',
        );
        await expect(page.getByRole('menuitem', { name: /^Delete task/ })).toHaveAttribute(
          'aria-disabled',
          'true',
        );
      } else {
        await expect(page.getByRole('treeitem', { name: /Sibling Agent/ })).toBeVisible();
        await page.getByRole('button', { name: 'Options for Preparing Agent' }).click();
      }
      const review = page.getByRole('menuitem', { name: /^Review environment/ });
      await expect(review).toContainText('all Agents inside it');
      await review.click();
    }
    await expect(page).toHaveURL(`/sandboxes/${boundary.sandbox.id}`);
    expect(writes).toEqual([]);
    const more = page.getByRole('button', { name: 'More actions', exact: true });
    await more.click();
    await page.getByRole('menuitem', { name: /^Delete Sandbox/ }).click();
    const confirmation = page.getByRole('dialog', { name: 'Delete Sandbox', exact: true });
    await expect(confirmation).toContainText(
      'This deletes the whole Sandbox environment, including all Agents, Workspaces, and files inside it.',
    );
    if ('activeOnArrival' in scenario) {
      expect(boundary.sandbox.status).toBe('active');
      await expect(confirmation).not.toContainText('cancel setup');
    }
    expect(writes).toEqual([]);
    await expect(
      confirmation.getByRole('button', { name: 'Request deletion', exact: true }),
    ).toBeDisabled();
    await confirmation.getByRole('button', { name: 'Keep environment', exact: true }).click();
    await expect(confirmation).toHaveCount(0);
    expect(writes).toEqual([]);
    await more.click();
    await page.getByRole('menuitem', { name: /^Delete Sandbox/ }).click();
    await expect(confirmation.getByRole('checkbox')).not.toBeChecked();
    await expect(
      confirmation.getByRole('button', { name: 'Request deletion', exact: true }),
    ).toBeDisabled();
    await confirmation.getByRole('checkbox').check();
    await confirmation.getByRole('button', { name: 'Request deletion', exact: true }).dblclick();
    if (scenario.uncertain) {
      await expect(page.getByRole('dialog', { name: 'Deleting Sandbox' })).toContainText(
        'not been confirmed',
      );
      await expect(
        page.getByRole('alert').filter({ hasText: 'Delete response unavailable' }),
      ).toBeVisible();
      expect(deletes).toHaveLength(1);
      await page.reload();
      const retry = page.getByRole('button', { name: 'Check request', exact: true });
      await expect(retry).toBeEnabled();
      await expect(page.getByRole('dialog', { name: 'Sandbox deleted', exact: true })).toHaveCount(
        0,
      );
      expect(deletes).toHaveLength(1);
      await retry.click();
    }
    await expect(page.getByText('Deleting 1 environment')).toBeVisible();
    expect(deletes).toHaveLength(scenario.uncertain ? 2 : 1);
    expect(deletes[0]!.key).toBeTruthy();
    if (scenario.uncertain) expect(deletes[1]).toEqual(deletes[0]);
    const detailFirst = scenario.entry === 'panel';
    if (detailFirst) await page.clock.install();
    // Reload from Agent Manager so the Sandbox page's detail refresh cannot settle the receipt
    // before the reopened status dialog observes retirement.
    await page.goto('/agent-manager');
    await page.reload();
    await page.getByRole('button', { name: 'View deletion status', exact: true }).click();
    const reopen = page.getByRole('button', { name: /View Sandbox deletion status/ });
    await expect(reopen).toBeEnabled();
    if (detailFirst) await page.clock.pauseAt(new Date(Date.now() + 500));
    await reopen.press('Enter');
    const progress = page.getByRole('dialog', { name: 'Deleting Sandbox', exact: true });
    await expect(progress).toContainText(
      'Check Environments for your current Sandbox capacity while deletion finishes.',
    );
    await expect(progress.getByText('Last status read:')).toBeVisible();
    expect(deletes).toHaveLength(scenario.uncertain ? 2 : 1);
    await expect(page.getByRole('dialog', { name: 'Sandbox deleted', exact: true })).toHaveCount(0);
    boundary.sandbox = { ...boundary.sandbox, status: 'retired' };
    if (detailFirst) {
      // Keep the dialog's poll paused while the original detail page reads terminal state.
      const detail = page.waitForResponse(
        (response) =>
          new URL(response.url()).pathname === `/api/v1/sandboxes/${boundary.sandbox.id}` &&
          response.request().method() === 'GET' &&
          response.ok(),
      );
      await page.evaluate((path) => {
        window.dispatchEvent(new CustomEvent('cma:route-request', { detail: path }));
      }, `/sandboxes/${boundary.sandbox.id}`);
      expect(await (await detail).json()).toMatchObject({
        id: boundary.sandbox.id,
        status: 'retired',
      });
      await expect(page.locator('.deletion-notice')).toHaveCount(0);
      await page.clock.resume();
    }
    await expect(page.getByRole('dialog', { name: 'Sandbox deleted', exact: true })).toBeVisible();
    expect(writes).toEqual(
      Array(scenario.uncertain ? 2 : 1).fill(`DELETE /api/v1/sandboxes/${boundary.sandbox.id}`),
    );
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}
