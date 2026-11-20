import { expect, test } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

for (const uncertain of [false, true]) {
  test(`LOCAL header Stop then separate Delete with uncertain stop=${uncertain}`, async ({
    page,
    baseURL,
  }) => {
    const boundary = new LocalStartup();
    boundary.agent = { ...boundary.agent, status: 'ready' };
    const sibling = {
      ...boundary.workspace.agents[0]!,
      id: 'agt_sibling',
      worktree_id: 'wkt_sibling',
      title: 'Sibling Agent',
    };
    boundary.workspace = {
      ...boundary.workspace,
      agents: [{ ...boundary.workspace.agents[0]!, status: 'ready' }, sibling],
    };
    await boundary.route(page, baseURL!);
    await page.route('**/api/v1/agents/agt_sibling/channel-usage', (route) =>
      route.fulfill({
        json: {
          agent_id: sibling.id,
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
          agent_id: sibling.id,
          created_at: boundary.agent.created_at,
          creation_origin: 'legacy_unknown',
        },
      }),
    );
    const stops: { path: string; key?: string; body: string | null }[] = [];
    const deletes: string[] = [];
    await page.route(`**/api/v1/agents/${boundary.agent.id}/stop`, async (route) => {
      const request = route.request();
      expect(request.method()).toBe('POST');
      stops.push({
        path: new URL(request.url()).pathname,
        key: request.headers()['idempotency-key'],
        body: request.postData(),
      });
      return uncertain && stops.length === 1
        ? route.fulfill({
            status: 503,
            json: { code: 'unavailable', detail: 'Stop response unavailable.' },
          })
        : route.fulfill({ status: 202, body: '' });
    });
    await page.route(`**/api/v1/agents/${boundary.agent.id}`, async (route) => {
      if (route.request().method() !== 'DELETE') return route.fallback();
      const key = route.request().headers()['idempotency-key'];
      expect(key).toBeTruthy();
      deletes.push(key!);
      return route.fulfill({ status: 202, body: '' });
    });
    await page.goto(`/agents/${boundary.agent.id}`);
    const more = page.getByRole('button', { name: 'More Agent actions', exact: true });
    await more.click();
    const remove = page.getByRole('menuitem', { name: /^Delete task/ });
    await expect(remove).toHaveAttribute('aria-disabled', 'true');
    await expect(remove).toContainText('Stop this Agent before deleting it and its Worktree.');
    await page.getByRole('menuitem', { name: 'Stop agent', exact: true }).click();
    const confirmation = page.getByRole('dialog', { name: 'Stop Agent', exact: true });
    await expect(confirmation).toContainText('Worktree are retained');
    expect(stops).toEqual([]);
    await confirmation.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(confirmation).toHaveCount(0);
    expect(stops).toEqual([]);
    await more.click();
    await page.getByRole('menuitem', { name: 'Stop agent', exact: true }).press('Enter');
    await confirmation.getByRole('button', { name: 'Stop Agent', exact: true }).dblclick();
    const progress = page.getByRole('dialog', { name: 'Stopping Agent', exact: true });
    await expect(progress).toBeVisible();
    if (uncertain) {
      await expect(progress.getByRole('alert')).toContainText('Stop response unavailable');
      expect(stops).toHaveLength(1);
      await progress.getByRole('button', { name: 'Retry stop request' }).click();
    }
    await expect(progress.getByText('Accepted', { exact: true })).toBeVisible();
    expect(stops).toHaveLength(uncertain ? 2 : 1);
    expect(stops[0]!.key).toBeTruthy();
    if (uncertain) expect(stops[1]).toEqual(stops[0]);
    expect(deletes).toEqual([]);
    await expect(page.getByRole('dialog', { name: 'Agent stopped', exact: true })).toHaveCount(0);
    boundary.agent = { ...boundary.agent, status: 'stopped' };
    boundary.workspace = {
      ...boundary.workspace,
      agents: [{ ...boundary.workspace.agents[0]!, status: 'stopped' }, sibling],
    };
    await page
      .getByRole('dialog', { name: 'Agent stopped', exact: true })
      .getByRole('button', { name: 'Done', exact: true })
      .click();
    expect(deletes).toEqual([]);
    await page.reload();
    await more.click();
    await expect(remove).not.toHaveAttribute('aria-disabled', 'true');
    await remove.click();
    const deletion = page.getByRole('dialog', { name: 'Delete Agent', exact: true });
    await expect(deletion).toContainText('and its Worktree');
    expect(deletes).toEqual([]);
    await deletion.getByRole('checkbox').check();
    await deletion.getByRole('button', { name: 'Delete Agent', exact: true }).click();
    await expect(
      page.getByText('Agent delete request accepted. Cleanup continues in the background.'),
    ).toBeVisible();
    expect(deletes).toHaveLength(1);
    await page.reload();
    await page.getByRole('button', { name: /View Agent deletion status/ }).click();
    await expect(page.getByRole('dialog', { name: 'Deleting Agent', exact: true })).toBeVisible();
    boundary.agent = {
      ...boundary.agent,
      status: 'deleted',
      worktree: { ...boundary.agent.worktree, status: 'removed' },
    };
    boundary.workspace = { ...boundary.workspace, agents: [sibling] };
    await expect(page.getByRole('dialog', { name: 'Agent deleted', exact: true })).toBeVisible();
    await page
      .getByRole('dialog', { name: 'Agent deleted', exact: true })
      .getByRole('button', { name: 'Done', exact: true })
      .click();
    await page.reload();
    await expect(page.locator(`.tree-row[data-node-id="agent:${boundary.agent.id}"]`)).toHaveCount(
      0,
    );
    await expect(page.locator('.tree-row[data-node-id="agent:agt_sibling"]')).toBeVisible();
    expect(boundary.sandbox.status).toBe('active');
    expect(deletes).toHaveLength(1);
    expect(stops).toHaveLength(uncertain ? 2 : 1);
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}

test('LOCAL stopped Agent stays stopped across an explicit Sandbox Resume', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  boundary.agent = { ...boundary.agent, status: 'stopped' };
  boundary.workspace = {
    ...boundary.workspace,
    agents: [{ ...boundary.workspace.agents[0]!, status: 'stopped' }],
  };
  boundary.sandbox = {
    ...boundary.sandbox,
    status: 'paused',
    lifecycle: { pause: { allowed: false, reason: 'not_active' }, revive: { allowed: true } },
  };
  await boundary.route(page, baseURL!);
  const resumes: string[] = [];
  await page.route(`**/api/v1/sandboxes/${boundary.sandbox.id}/revive`, async (route) => {
    expect(route.request().method()).toBe('POST');
    expect(route.request().headers()['idempotency-key']).toBeTruthy();
    resumes.push(route.request().url());
    boundary.sandbox = {
      ...boundary.sandbox,
      status: 'reviving',
      lifecycle: {
        pause: { allowed: false, reason: 'not_active' },
        revive: { allowed: false, reason: 'not_paused' },
        operation: {
          purpose: 'revive',
          phase: 'home_restore',
          started_at: '2026-09-05T00:00:00Z',
        },
      },
    };
    await route.fulfill({ status: 202, json: boundary.sandbox });
  });
  for (const phase of ['paused', 'reviving', 'active'] as const) {
    await page.goto(`/agents/${boundary.agent.id}`);
    const sidebar = page.locator(`.tree-row[data-node-id="agent:${boundary.agent.id}"]`);
    for (const [anchor, label] of [
      [page.getByRole('button', { name: 'More Agent actions', exact: true }), 'Stop agent'],
      [sidebar.getByRole('button', { name: /^Options for / }), 'Stop'],
    ] as const) {
      await anchor.click();
      const stop = page.getByRole('menuitem', {
        name: `${label} Agent is already stopped.`,
        exact: true,
      });
      await expect(stop).toHaveAttribute('aria-disabled', 'true');
      await expect(page.getByRole('menuitem', { name: /^Delete/ })).toHaveAttribute(
        'aria-disabled',
        String(phase !== 'active'),
      );
      await expect(
        page.getByRole('menuitem', { name: 'Export history', exact: true }),
      ).not.toHaveAttribute('aria-disabled', 'true');
      await stop.click({ force: true });
      await stop.press('Enter');
      await stop.press('Space');
      await expect(page.getByRole('dialog', { name: 'Stop Agent', exact: true })).toHaveCount(0);
      await page.keyboard.press('Escape');
    }
    expect(boundary.agent.status).toBe('stopped');
    expect(boundary.writes).toEqual([]);
    if (phase === 'paused') {
      await page.goto(`/sandboxes/${boundary.sandbox.id}`);
      await page.getByRole('button', { name: 'Resume', exact: true }).click();
      await expect(page.getByText('Resume request accepted.', { exact: true })).toBeVisible();
      expect(resumes).toHaveLength(1);
      expect(boundary.sandbox.status).toBe('reviving');
    } else if (phase === 'reviving') {
      boundary.sandbox = {
        ...boundary.sandbox,
        status: 'active',
        lifecycle: { pause: { allowed: true }, revive: { allowed: false, reason: 'not_paused' } },
      };
    }
  }
  expect(resumes).toHaveLength(1);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
