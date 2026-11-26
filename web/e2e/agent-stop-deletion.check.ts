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

test('LOCAL owner Resume checks the paused Sandbox before one keyed request', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  boundary.sandbox = {
    ...boundary.sandbox,
    status: 'paused',
    lifecycle: {
      pause: { allowed: false, reason: 'not_active' },
      revive: { allowed: true },
    },
  };
  boundary.agent = {
    ...boundary.agent,
    status: 'stopped',
    sandbox: { ...boundary.agent.sandbox, status: 'paused' },
  };
  await boundary.route(page, baseURL!);
  let release!: () => void;
  const connection = new Promise<void>((complete) => (release = complete));
  const reviveKeys: string[] = [];
  let checks = 0;
  await page.route('**/auth/connections/check', async (route) => {
    const { target } = route.request().postDataJSON() as { target: { type: string; id?: string } };
    if (target.type !== 'sandbox') return route.fallback();
    expect(target.id).toBe(boundary.sandbox.id);
    checks += 1;
    await connection;
    return route.fulfill({
      json: { owner: 'local-owner', ready: true, checked_at: '2026-11-21T00:00:00Z', services: [] },
    });
  });
  await page.route(`**/api/v1/sandboxes/${boundary.sandbox.id}/revive`, async (route) => {
    expect(route.request().method()).toBe('POST');
    const key = route.request().headers()['idempotency-key'];
    expect(key).toBeTruthy();
    reviveKeys.push(key!);
    boundary.sandbox = {
      ...boundary.sandbox,
      status: 'active',
      lifecycle: { pause: { allowed: true }, revive: { allowed: false, reason: 'not_paused' } },
    };
    boundary.agent = {
      ...boundary.agent,
      sandbox: { ...boundary.agent.sandbox, status: 'active' },
    };
    return route.fulfill({ status: 202, json: boundary.sandbox });
  });
  const expectStoppedInBothMenus = async () => {
    await page.goto(`/agents/${boundary.agent.id}`);
    await navigation(page, true);
    for (const [menu, label] of [
      [page.getByRole('button', { name: 'More Agent actions', exact: true }), 'Stop agent'],
      [
        resourceRow(page, 'agent', boundary.agent.id).getByRole('button', {
          name: /^Options for /,
        }),
        'Stop',
      ],
    ] as const) {
      await menu.click();
      const stop = page.getByRole('menuitem', {
        name: `${label} Agent is already stopped.`,
        exact: true,
      });
      await expect(stop).toHaveAttribute('aria-disabled', 'true');
      await stop.click({ force: true });
      await stop.press('Enter');
      await stop.press('Space');
      await expect(page.getByRole('dialog', { name: 'Stop Agent', exact: true })).toHaveCount(0);
      await page.keyboard.press('Escape');
    }
    noEffects(boundary);
  };
  await expectStoppedInBothMenus();
  await page.goto(`/sandboxes/${boundary.sandbox.id}`);
  const resume = page.getByRole('button', { name: 'Resume', exact: true });
  await expect(resume).toBeEnabled();
  expect(checks).toBe(0);
  noEffects(boundary);
  await resume.click();
  await expect.poll(() => checks).toBe(1);
  await expect(resume).toBeDisabled();
  expect(reviveKeys).toEqual([]);
  release();
  await expect(page.getByText('Sandbox resumed.')).toBeVisible();
  expect(reviveKeys).toHaveLength(1);
  await expect(page.getByRole('button', { name: 'Pause', exact: true })).toBeEnabled();

  await expectStoppedInBothMenus();
  await page.getByRole('button', { name: 'More Agent actions', exact: true }).click();
  const remove = page.getByRole('menuitem', { name: /^Delete task\b/ });
  await expect(remove).toBeEnabled();
  await remove.click();
  await expect(page.getByRole('dialog', { name: 'Delete Agent', exact: true })).toBeVisible();
  noEffects(boundary);
});

for (const { combined, uncertain } of [
  { combined: false, uncertain: false },
  { combined: false, uncertain: true },
  { combined: true, uncertain: false },
  { combined: true, uncertain: true },
]) {
  test(`LOCAL header ${combined ? 'Stop and delete' : 'standalone Stop then Delete'} with uncertain response=${uncertain}`, async ({
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
    const siblingPath = `/api/v1/agents/${sibling.id}`;
    const siblingReads = {
      [siblingPath]: {
        ...boundary.agent,
        id: sibling.id,
        status: sibling.status,
        title: sibling.title,
        worktree: { ...boundary.agent.worktree, id: sibling.worktree_id, agent_id: sibling.id },
      },
      [`${siblingPath}/transcript`]: { agent_id: sibling.id, through_seq: 0, turns: [] },
      [`${siblingPath}/usage`]: {
        agent_id: sibling.id,
        through_seq: 0,
        input_tokens: 0,
        output_tokens: 0,
        context_used: 0,
      },
      [`${siblingPath}/events`]: { items: [] },
    };
    await boundary.route(page, baseURL!);
    await page.route(
      (url) => url.origin === baseURL && Object.hasOwn(siblingReads, url.pathname),
      (route) =>
        route.request().method() === 'GET'
          ? route.fulfill({ json: siblingReads[new URL(route.request().url()).pathname] })
          : route.fallback(),
    );
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
      expect(boundary.agent.status).toBe('stopped');
      if (combined && uncertain && deletes.length === 1)
        return route.fulfill({
          status: 503,
          json: { code: 'unavailable', detail: 'Delete response unavailable.' },
        });
      return route.fulfill({ status: 202, body: '' });
    });
    await page.goto(`/agents/${boundary.agent.id}`);
    const more = page.getByRole('button', { name: 'More Agent actions', exact: true });
    await more.click();
    const remove = page.getByRole('menuitem', { name: /^Delete task/ });
    await expect(remove).toHaveAttribute('aria-disabled', 'false');
    const action = combined
      ? remove
      : page.getByRole('menuitem', { name: 'Stop agent', exact: true });
    await action.click();
    const confirmation = page.getByRole('dialog', {
      name: combined ? 'Delete Agent' : 'Stop Agent',
      exact: true,
    });
    await expect(confirmation).toContainText(
      combined ? 'and its Worktree' : 'Worktree are retained',
    );
    expect(stops).toEqual([]);
    await confirmation.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(confirmation).toHaveCount(0);
    expect(stops).toEqual([]);
    expect(deletes).toEqual([]);
    await more.click();
    await action.press('Enter');
    if (combined) await confirmation.getByRole('checkbox').check();
    await confirmation
      .getByRole('button', { name: combined ? 'Stop and delete' : 'Stop Agent', exact: true })
      .dblclick();
    const progress = page.getByRole('dialog', {
      name: combined ? 'Deleting Agent' : 'Stopping Agent',
      exact: true,
    });
    await expect(progress).toBeVisible();
    if (uncertain) {
      await expect(progress.getByRole('alert')).toContainText('Stop response unavailable');
      expect(stops).toHaveLength(1);
      await progress.getByRole('button', { name: 'Retry stop request' }).click();
    }
    await expect(
      progress.getByText(combined ? 'Stopping' : 'Accepted', { exact: true }),
    ).toBeVisible();
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
    if (!combined) {
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
    } else if (uncertain) {
      await expect(progress.getByRole('alert')).toContainText('Delete response unavailable');
      expect(deletes).toHaveLength(1);
      await page.reload();
      await expect(progress).toBeVisible();
      expect(deletes).toHaveLength(1);
      await progress.getByRole('button', { name: 'Check request' }).click();
      await expect.poll(() => deletes.length).toBe(2);
      expect(deletes[1]).toBe(deletes[0]);
    }
    if (combined && uncertain) {
      await expect(progress).toHaveCount(0);
      await expect(page.getByText('Deleting 1 Agent')).toHaveCount(0);
    } else {
      await expect(page.getByText('Deleting 1 Agent')).toBeVisible();
    }
    expect(deletes).toHaveLength(combined && uncertain ? 2 : 1);
    expect(deletes[0]).not.toBe(stops[0]!.key);
    const detailFirst = !combined && !uncertain;
    if (detailFirst) await page.clock.install();
    // Reload from Agent Manager so the Agent page's detail refresh cannot settle the receipt
    // before the reopened status dialog observes cleanup.
    await page.goto('/agent-manager');
    await page.reload();
    await page.getByRole('button', { name: 'View deletion status', exact: true }).click();
    const reopen = page.getByRole('button', { name: /View Agent deletion status/ });
    await expect(reopen).toBeEnabled();
    if (detailFirst) await page.clock.pauseAt(new Date(Date.now() + 500));
    await reopen.press('Enter');
    const deleting = page.getByRole('dialog', { name: 'Deleting Agent', exact: true });
    await expect(deleting.getByText('Last status read:')).toBeVisible();
    const deleted = page.getByRole('dialog', { name: 'Agent deleted', exact: true });
    await expect(deleted).toHaveCount(0);
    boundary.agent = {
      ...boundary.agent,
      status: 'deleted',
      worktree: { ...boundary.agent.worktree, status: 'removed' },
    };
    boundary.workspace = { ...boundary.workspace, agents: [sibling] };
    if (detailFirst) {
      // Keep the dialog's poll paused while the original detail page reads terminal state.
      const detail = page.waitForResponse(
        (response) =>
          new URL(response.url()).pathname === `/api/v1/agents/${boundary.agent.id}` &&
          response.request().method() === 'GET' &&
          response.ok(),
      );
      await page.evaluate((path) => {
        window.dispatchEvent(new CustomEvent('cma:route-request', { detail: path }));
      }, `/agents/${boundary.agent.id}`);
      expect(await (await detail).json()).toMatchObject({
        id: boundary.agent.id,
        status: 'deleted',
        worktree: { status: 'removed' },
      });
      await expect(page.locator('.deletion-notice')).toHaveCount(0);
      await page.clock.resume();
    }
    await expect(deleted).toBeVisible();
    await deleted.getByRole('button', { name: 'Done', exact: true }).click();
    await page.reload();
    await expect(page.locator(`.tree-row[data-node-id="agent:${boundary.agent.id}"]`)).toHaveCount(
      0,
    );
    await expect(page.locator('.tree-row[data-node-id="agent:agt_sibling"]')).toBeVisible();
    expect(boundary.sandbox.status).toBe('active');
    expect(deletes).toHaveLength(combined && uncertain ? 2 : 1);
    expect(stops).toHaveLength(uncertain ? 2 : 1);
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}
