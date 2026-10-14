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
for (const target of ['new', 'contextual'] as const) {
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
        await expect(page.getByLabel('Agent location')).toHaveCount(0);
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
  await page.route('**/api/v1/agents/agt_contract/models', (route) =>
    route.fulfill({ json: { models: [{ id: 'gpt-6', label: 'GPT-6', available: true }] } }),
  );
  await page.route('**/api/v1/agents/agt_contract/skills', (route) =>
    route.fulfill({ json: { items: [], pending: false, can_install: true } }),
  );
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
  await expect(details.getByLabel('Model', { exact: true })).toHaveText('GPT-6');
  await details.getByRole('button', { name: 'Reasoning effort', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Medium', exact: true }).click();
  await details.getByRole('button', { name: 'Permission mode', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Guarded', exact: true }).click();
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

test('LOCAL Details uses model and skill catalogs in bounded responsive containers and confirms stop progress', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  await page.route('**/api/v1/agents/agt_contract/models', (route) =>
    route.fulfill({
      json: {
        models: [
          { id: 'gpt-6', label: 'GPT-6', available: true },
          { id: 'fast-model', label: 'Fast model', available: true },
        ],
      },
    }),
  );
  await page.route('**/api/v1/profile-authoring/services', (route) =>
    route.fulfill({
      json: Array.from({ length: 105 }, (_, i) => ({
        id: `service-${i}`,
        name: `Accessible service ${i}`,
        description: 'A service available to this account.',
      })),
    }),
  );
  await page.route('**/api/v1/profile-authoring/skills*', (route) =>
    route.fulfill({
      json: {
        items: Array.from({ length: 20 }, (_, i) => ({
          id: `skill-${i}`,
          name: `Skill ${i}`,
          description: 'An Ornn skill available to this account.',
          versions: ['1.2', '1.1'],
        })),
      },
    }),
  );
  let skillStatus: 'pending' | 'installed' | undefined;
  const installs: { key?: string; body: unknown }[] = [];
  await page.route('**/api/v1/agents/agt_contract/skills', (route) => {
    if (route.request().method() === 'POST') {
      installs.push({
        key: route.request().headers()['idempotency-key'],
        body: route.request().postDataJSON(),
      });
      skillStatus = 'pending';
    }
    return route.fulfill({
      status: route.request().method() === 'POST' ? 202 : 200,
      json: {
        items: skillStatus
          ? [{ id: 'skill-0', version: '1.2', source: 'ornn', status: skillStatus }]
          : [],
        pending: skillStatus === 'pending',
        can_install: skillStatus !== 'pending',
      },
    });
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/agents/agt_contract');
  await page.getByRole('button', { name: 'Show Agent details' }).click();
  const details = page.getByRole('complementary', { name: 'Agent details' });
  await expect(details.getByRole('button', { name: 'Model', exact: true })).toHaveText('GPT-6');
  await details.getByRole('button', { name: 'Model', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Fast model', exact: true }).click();
  await expect(details.getByRole('button', { name: 'Model', exact: true })).toHaveText(
    'Fast model',
  );
  await expect(details.getByRole('list', { name: 'Services' }).getByRole('listitem')).toHaveCount(
    108,
  );
  await expect(
    details.getByRole('list', { name: 'Available Ornn skills' }).getByRole('listitem'),
  ).toHaveCount(20);
  for (const width of [1440, 390, 320]) {
    await page.setViewportSize({ width, height: 1000 });
    await expect(details).toBeVisible();
    await details.getByRole('button', { name: 'Model', exact: true }).scrollIntoViewIfNeeded();
    for (const name of ['Services', 'Available Ornn skills']) {
      const list = details.getByRole('list', { name, exact: true });
      expect(
        await list.evaluate(
          (el) =>
            el.clientHeight <= 320 &&
            el.scrollHeight > el.clientHeight &&
            el.scrollWidth <= el.clientWidth + 1,
        ),
      ).toBe(true);
    }
    expect(await details.evaluate((el) => el.scrollWidth <= el.clientWidth + 1)).toBe(true);
    await page.screenshot({
      path: info.outputPath(`details-catalogs-${width}.png`),
      fullPage: true,
      animations: 'disabled',
    });
    await details
      .getByRole('list', { name: 'Available Ornn skills', exact: true })
      .scrollIntoViewIfNeeded();
    await page.screenshot({
      path: info.outputPath(`details-skills-${width}.png`),
      fullPage: true,
      animations: 'disabled',
    });
  }
  await details.getByRole('button', { name: 'Install Skill 0', exact: true }).click();
  await expect(details.getByText('Waiting for the Agent to confirm installation.')).toBeVisible();
  await expect(details.getByText('1.2 · Installed', { exact: true })).toHaveCount(0);
  skillStatus = 'installed';
  await expect(details.getByText('1.2 · Installed', { exact: true })).toBeVisible();
  expect(installs).toHaveLength(1);
  expect(installs[0]?.body).toEqual({ id: 'skill-0', version: '1.2' });
  expect(installs[0]?.key).toBeTruthy();
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.getByRole('button', { name: 'Close Agent details' }).click();
  let stops = 0;
  await page.route('**/api/v1/agents/agt_contract/stop', (route) => {
    stops++;
    return route.fulfill({ status: 202, body: '' });
  });
  await page
    .locator('.tree-row[data-node-id="agent:agt_contract"]')
    .getByRole('button', { name: /Options for/ })
    .click();
  await page.getByRole('menuitem', { name: 'Stop', exact: true }).click();
  await expect(page.getByRole('dialog', { name: 'Stopping Agent' })).toBeVisible();
  await expect(page.getByText('Accepted', { exact: true })).toBeVisible();
  expect(stops).toBe(1);
  await expect(page.getByRole('dialog', { name: 'Agent stopped', exact: true })).toHaveCount(0);
  boundary.agent = { ...boundary.agent, status: 'stopped' };
  await expect(page.getByRole('dialog', { name: 'Agent stopped', exact: true })).toBeVisible();
  await capture(page, info, 'stop-confirmed');
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
