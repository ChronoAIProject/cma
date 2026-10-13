import { test, expect, type Page, type TestInfo } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

async function capture(page: Page, info: TestInfo, state: string) {
  for (const width of [1440, 390, 320]) {
    await page.setViewportSize({ width, height: 900 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
      true,
    );
    await expect
      .poll(() =>
        page.getByRole('dialog').evaluate((dialog) => {
          const bounds = dialog.getBoundingClientRect();
          return (
            bounds.left >= 0 &&
            bounds.right <= innerWidth + 1 &&
            dialog.scrollWidth <= dialog.clientWidth + 1
          );
        }),
      )
      .toBe(true);
    await page.screenshot({
      path: info.outputPath(`${state}-${width}.png`),
      fullPage: true,
      animations: 'disabled',
    });
  }
}
for (const target of ['new', 'existing', 'contextual'] as const) {
  test(`LOCAL ${target} Sandbox creation uses only repository context and recovers the same command after reload`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
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
      if (target === 'contextual')
        await page.getByRole('button', { name: 'New agent in this Sandbox' }).click();
      else {
        await page
          .locator('.sidebar')
          .getByRole('button', { name: 'New agent', exact: true })
          .click();
        if (target === 'existing')
          await page.getByLabel('Agent location').selectOption('sbx_route');
      }
    };
    await open();
    const dialog = page.getByRole('dialog', { name: 'New agent', exact: true });
    await expect(dialog.getByLabel('GitHub repository (optional)')).toBeVisible();
    expect(await dialog.locator('input').count()).toBe(1);
    await dialog.getByLabel('GitHub repository (optional)').fill('https://github.com/org/research');
    await capture(page, info, `${target}-creation`);
    await dialog.getByRole('button', { name: 'Create agent', exact: true }).click();
    await expect(
      dialog.getByRole('button', { name: 'Recover creation', exact: true }),
    ).toBeEnabled();
    await page.setViewportSize({ width: 1440, height: 900 });
    await open();
    await dialog.getByRole('button', { name: 'Recover creation', exact: true }).click();
    await expect(dialog.getByRole('button', { name: 'Open agent', exact: true })).toBeDisabled();
    expect(writes).toHaveLength(2);
    expect(writes[1]).toEqual(writes[0]);
    expect(writes[0]?.key).toBeTruthy();
    expect(writes[0]?.body).toEqual({
      name: 'research',
      repository: { url: 'https://github.com/org/research' },
      ...(target === 'new' ? {} : { sandbox_id: 'sbx_route' }),
    });
    ready = true;
    await dialog.getByRole('button', { name: 'Open agent', exact: true }).click();
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
  await dialog.getByRole('button', { name: 'Approve and connect' }).click();
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

test('LOCAL Agent detail settings apply only after a confirmed runtime response and replay an uncertain command', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  const settings = {
    model: 'gpt-6',
    reasoning_effort: 'high',
    permission_mode: 'autonomous',
    pending: false,
  };
  const writes: { key?: string; body: unknown }[] = [];
  await page.route('**/api/agent-manager/v1/agents/agt_contract/settings', (route) => {
    if (route.request().method() === 'GET') return route.fulfill({ json: settings });
    writes.push({
      key: route.request().headers()['idempotency-key'],
      body: route.request().postDataJSON(),
    });
    if (writes.length === 1) return route.abort('connectionfailed');
    return route.fulfill({
      json: { ...settings, reasoning_effort: 'medium', permission_mode: 'guarded' },
    });
  });
  await page.goto('/agents/agt_contract');
  await page.getByRole('button', { name: 'Show Agent details' }).click();
  const details = page.getByRole('complementary', { name: 'Agent details' });
  await expect(details.getByLabel('Model', { exact: true })).toHaveValue('gpt-6');
  await details.getByLabel('Reasoning effort').selectOption('medium');
  await details.getByLabel('Permission mode').selectOption('guarded');
  await details.getByRole('button', { name: 'Apply settings', exact: true }).click();
  await expect(details.getByRole('button', { name: 'Retry settings update' })).toBeEnabled();
  await expect(details.getByLabel('Model', { exact: true })).toBeDisabled();
  await expect(details.getByText('Settings applied to this Agent.')).toHaveCount(0);
  await details.getByRole('button', { name: 'Retry settings update' }).click();
  await expect(details.getByText('Settings applied to this Agent.')).toBeVisible();
  expect(writes).toHaveLength(2);
  expect(writes[1]).toEqual(writes[0]);
  expect(writes[0]?.body).toEqual({
    model: 'gpt-6',
    reasoning_effort: 'medium',
    permission_mode: 'guarded',
  });
  expect(writes[0]?.key).toBeTruthy();
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
