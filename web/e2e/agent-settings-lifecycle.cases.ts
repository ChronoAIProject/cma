import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { capture } from './agent-settings-layout.fixture';

for (const target of ['new', 'contextual'] as const) {
  test(`LOCAL ${target} task creation preserves repository or inherited context and recovers the same command after reload`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    await page.route('**/api/v2/ui/task-experience', (route) =>
      route.fulfill({
        json: {
          object: 'cma.ui.task_experience',
          schema_version: 1,
          account_id: 'local-owner',
          capabilities: {
            task_home_admission: true,
            desktop_enable: false,
            suggested_followup_admission: false,
            browser_takeover: false,
            replay_presentation: false,
          },
        },
      }),
    );
    const writes: { body: unknown; key?: string }[] = [];
    let ready = false;
    await page.route('**/api/agent-manager/v1/agents', async (route) => {
      writes.push({
        body: route.request().postDataJSON(),
        key: route.request().headers()['idempotency-key'],
      });
      if (writes.length === 1) return route.abort('connectionfailed');
      return route.fulfill({ json: { id: 'lifecycle-browser', status: 'queued' }, status: 202 });
    });
    await page.route('**/api/agent-manager/v1/agent-lifecycle/lifecycle-browser', (route) =>
      route.fulfill({
        json: {
          id: 'lifecycle-browser',
          status: ready ? 'succeeded' : 'running',
          phase: 'workspace_runtime',
          agent_id: ready ? 'agt_contract' : undefined,
          events: [
            {
              sequence: 1,
              phase: 'sandbox_ready',
              status: 'ready',
              message: 'Sandbox ready',
              occurred_at: '2026-10-13T00:00:00Z',
            },
          ],
        },
      }),
    );
    const open = async () => {
      await page.goto('/sandboxes/sbx_route');
      if (target === 'contextual') {
        await page
          .locator('.sandbox-page')
          .getByRole('button', { name: 'More actions', exact: true })
          .click();
        await page.getByRole('menuitem', { name: 'New agent', exact: true }).click();
      } else {
        await page
          .locator('.sidebar')
          .getByRole('button', { name: 'New agent', exact: true })
          .click();
        await expect(page.getByLabel('Agent location')).toHaveCount(0);
      }
    };
    await open();
    const dialog =
      target === 'new'
        ? page.locator('.task-home')
        : page.getByRole('dialog', { name: 'New task', exact: true });
    await expect(dialog.getByLabel('Your task')).toBeVisible();
    await expect(dialog.getByRole('radiogroup')).toHaveCount(0);
    await expect(dialog.getByText('Where it works', { exact: true })).toHaveCount(0);
    await dialog
      .getByLabel('Your task')
      .fill('  Inspect the selected source\nKeep this exact task.  ');
    if (target === 'new') {
      await dialog.getByRole('button', { name: 'Add git repo' }).click();
      const repository = page.getByRole('dialog', { name: 'Add git repo' });
      await repository.getByLabel('GitHub repository').fill('org/research.git/');
      await repository.getByLabel('Repository ref').fill('proof/fixed');
      await repository.getByRole('button', { name: 'Add repo', exact: true }).click();
    } else {
      await expect(dialog.getByLabel('GitHub repository')).toHaveCount(0);
      await expect(dialog.getByText(/Repository context is inherited/)).toBeVisible();
    }
    await capture(page, info, `${target}-creation`, dialog);
    await dialog.getByRole('button', { name: 'Start task', exact: true }).click();
    await expect(
      dialog.getByRole('button', { name: 'Recover original task', exact: true }),
    ).toBeEnabled();
    await page.setViewportSize({ width: 1440, height: 900 });
    await open();
    const original = dialog.getByRole('region', { name: 'Original request', exact: true });
    await expect(original).toBeVisible();
    expect(await original.locator('p').textContent()).toBe(
      '  Inspect the selected source\nKeep this exact task.  ',
    );
    await dialog.getByRole('button', { name: 'Recover original task', exact: true }).click();
    await expect(
      dialog.getByRole('heading', { name: 'Getting its cloud computer ready', exact: true }),
    ).toBeVisible();
    await expect(page).not.toHaveURL(/\/agents\/agt_contract$/);
    expect(writes).toHaveLength(2);
    expect(writes[1]).toEqual(writes[0]);
    expect(writes[0]?.key).toBeTruthy();
    expect(writes[0]?.body).toEqual({
      name: 'New task',
      initial_message: '  Inspect the selected source\nKeep this exact task.  ',
      ...(target === 'new'
        ? { repository: { url: 'https://github.com/org/research', ref: 'proof/fixed' } }
        : { sandbox_id: 'sbx_route' }),
    });
    ready = true;
    await expect(page).toHaveURL(/\/agents\/agt_contract$/);
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}

test('LOCAL access approval survives reload and OAuth readiness still waits for durable runtime success', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  let status = 'pending';
  const access = () => ({
    id: 'access-browser',
    agent_id: 'agt_contract',
    status,
    services: ['api-github'],
    reason: 'Clone the repository and prepare a pull request.',
    ...(status === 'connecting'
      ? { connection: { target: { type: 'agent_access_request', id: 'access-browser' } } }
      : {}),
  });
  await page.route('**/api/agent-manager/v1/agents/agt_contract/access-requests', (route) =>
    route.fulfill({ json: { items: [access()] } }),
  );
  const decisions: { key?: string; body: unknown }[] = [];
  await page.route(
    '**/api/agent-manager/v1/agents/agt_contract/access-requests/access-browser/approve',
    (route) => {
      decisions.push({
        key: route.request().headers()['idempotency-key'],
        body: route.request().postDataJSON(),
      });
      if (decisions.length === 1) return route.abort('connectionfailed');
      status = 'connecting';
      return route.fulfill({ json: access() });
    },
  );
  await page.goto('/agents/agt_contract');
  const dialog = page.getByRole('dialog', { name: 'Agent key access' });
  await expect(dialog).toHaveCount(1);
  await expect(dialog).toBeVisible();
  await expect(
    dialog.getByRole('checkbox', { name: 'chrono-llm-public Required', exact: true }),
  ).toBeDisabled();
  await expect(
    dialog.getByRole('checkbox', { name: 'ornn-api Required', exact: true }),
  ).toBeChecked();
  await capture(page, info, 'access-request');
  await dialog.getByRole('button', { name: 'Approve', exact: true }).click();
  await expect(dialog.getByRole('button', { name: 'Retry access decision' })).toBeVisible();
  await page.reload();
  await dialog.getByRole('button', { name: 'Retry access decision' }).click();
  await expect(dialog.getByText(/Verifying the updated Agent key/)).toBeVisible();
  expect(decisions).toHaveLength(2);
  expect(decisions[1]).toEqual(decisions[0]);
  expect(decisions[0]?.key).toBeTruthy();
  expect(decisions[0]?.body).toMatchObject({
    services: expect.arrayContaining(['chrono-llm-public', 'ornn-api', 'api-github']),
  });
  await page.keyboard.press('Escape');
  await page.getByRole('button', { name: 'Review access request' }).click();
  await expect(dialog.getByText(/Verifying the updated Agent key/)).toBeVisible();
  expect(decisions).toHaveLength(2);
  status = 'succeeded';
  await expect(dialog).toHaveCount(0);
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
