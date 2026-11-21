import { expect, test } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { navigation, noEffects, resourceRow } from './resource-lifecycle.fixture';

for (const source of ['header', 'task row'] as const) {
  test(`LOCAL paused task deletion opens its Sandbox from the ${source} without writing`, async ({
    page,
    baseURL,
  }) => {
    const boundary = new LocalStartup();
    const agentStatus = source === 'header' ? 'stopped' : 'failed';
    const canResume = source === 'header';
    boundary.sandbox = {
      ...boundary.sandbox,
      status: 'paused',
      lifecycle: {
        pause: { allowed: false, reason: 'not_active' },
        revive: canResume ? { allowed: true } : { allowed: false, reason: 'work_in_progress' },
      },
    };
    boundary.agent = {
      ...boundary.agent,
      status: agentStatus,
      sandbox: { ...boundary.agent.sandbox, status: 'paused' },
    };
    boundary.workspace = {
      ...boundary.workspace,
      agents: boundary.workspace.agents.map((agent) => ({ ...agent, status: agentStatus })),
    };
    await boundary.route(page, baseURL!);
    await page.goto(`/agents/${boundary.agent.id}`);
    if (source === 'task row') await navigation(page, true);
    const menu =
      source === 'header'
        ? page.getByRole('button', { name: 'More Agent actions', exact: true })
        : resourceRow(page, 'agent', boundary.agent.id).getByRole('button', {
            name: /^Options for /,
          });
    const remove = page.getByRole('menuitem', {
      name: source === 'header' ? /^Delete task\b/ : /^Delete\b/,
    });
    // The header can draw before the exact Sandbox observation arrives.
    await expect(async () => {
      if ((await menu.getAttribute('aria-expanded')) === 'true') await menu.press('Escape');
      await menu.press('Enter');
      await expect(remove).toContainText(
        'Resume the Sandbox before deleting this Agent and its Worktree.',
        { timeout: 500 },
      );
    }).toPass({ timeout: 10_000 });
    await expect(remove).toHaveAttribute('aria-disabled', 'true');
    const recovery = page.getByRole('menuitem', { name: 'Open Sandbox to resume', exact: true });
    await expect(recovery).toHaveAttribute('href', `/sandboxes/${boundary.sandbox.id}`);
    await recovery.press('Enter');
    await expect(page).toHaveURL(`${baseURL}/sandboxes/${boundary.sandbox.id}`);
    await expect(
      page.getByRole('heading', { name: boundary.sandbox.name, exact: true }),
    ).toBeVisible();
    const resume = page.getByRole('button', { name: 'Resume', exact: true });
    if (canResume) await expect(resume).toBeEnabled();
    else await expect(resume).toBeDisabled();
    noEffects(boundary);
  });
}

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
