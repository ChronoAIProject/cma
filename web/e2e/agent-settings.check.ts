import { test, expect, type Page, type TestInfo } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

test('LOCAL composer settings and message feedback remain usable across screen sizes and reload', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  const votes = new Map<string, 'up' | 'down'>();
  await page.route('**/api/v1/agents/agt_contract/message-feedback*', (route) =>
    route.fulfill({
      json: {
        items: [...votes].map(([message_id, vote]) => ({ message_id, vote })),
        next: null,
      },
    }),
  );
  await page.route('**/api/v1/agents/agt_contract/messages/*/feedback', (route) => {
    const id = decodeURIComponent(new URL(route.request().url()).pathname.split('/').at(-2)!);
    const { vote } = route.request().postDataJSON() as { vote: 'up' | 'down' | null };
    if (vote === null) votes.delete(id);
    else votes.set(id, vote);
    return route.fulfill({ json: { message_id: id, vote } });
  });
  await page.route('**/api/v1/agents/agt_contract/models', (route) =>
    route.fulfill({ json: { models: [{ id: 'gpt-6', label: 'GPT-6', available: true }] } }),
  );
  await page.route('**/api/v1/agents/agt_contract/transcript', (route) =>
    route.fulfill({
      json: {
        agent_id: 'agt_contract',
        through_seq: 4,
        turns: [
          {
            turn_sequence: 1,
            started_at: '2026-10-20T16:30:00Z',
            owner_message: 'Review the implementation.',
            agent_messages: ['The implementation preserves the original Agent conversation.'],
            messages: [
              {
                sequence: 2,
                role: 'user',
                content: 'Review the implementation.',
                appended_at: '2026-10-20T16:30:00Z',
              },
              {
                sequence: 3,
                role: 'assistant',
                content: 'The implementation preserves the original Agent conversation.',
                appended_at: '2026-10-20T16:31:00Z',
              },
            ],
            outcome: { type: 'completed', at: '2026-10-20T16:31:01Z' },
          },
        ],
      },
    }),
  );
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/agents/agt_contract');
  const composer = page.getByRole('form', { name: 'Message Agent' });
  await expect(composer.getByRole('button', { name: /^Model and reasoning effort/ })).toHaveText(
    'GPT-6 High',
  );
  const reply = page.getByRole('article', { name: 'Chrono Agent' });
  await expect(reply.getByText('21 Oct 2026, 00:31 SGT')).toBeVisible();
  await page.getByRole('textbox', { name: 'Message' }).fill('Preserve my unsent draft');
  const actions = reply.getByRole('group', { name: 'Message actions' });
  await page.locator('.agent-header__strip').hover();
  await expect
    .poll(() => actions.evaluate((element) => getComputedStyle(element).opacity))
    .toBe('0');
  await reply.hover();
  await expect
    .poll(() => actions.evaluate((element) => getComputedStyle(element).opacity))
    .toBe('1');
  await reply.getByRole('button', { name: 'Upvote message' }).click();
  await expect(reply.getByRole('button', { name: 'Upvote message' })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  await reply.getByRole('button', { name: 'Downvote message' }).click();
  await expect(reply.getByRole('button', { name: 'Upvote message' })).toHaveAttribute(
    'aria-pressed',
    'false',
  );
  await expect(reply.getByRole('button', { name: 'Downvote message' })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  for (const width of [1440, 768, 390, 320]) {
    await page.setViewportSize({ width, height: 1000 });
    await expect(
      composer.getByRole('button', { name: 'Permission mode', exact: true }),
    ).toBeVisible();
    await expect
      .poll(() =>
        composer.evaluate((element) => {
          const bounds = element.getBoundingClientRect();
          return bounds.left >= 0 && bounds.right <= innerWidth && bounds.width > 0;
        }),
      )
      .toBe(true);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
      true,
    );
    expect(await composer.evaluate((form) => form.scrollWidth <= form.clientWidth + 1)).toBe(true);
    await expect(page.getByRole('textbox', { name: 'Message' })).toHaveValue(
      'Preserve my unsent draft',
    );
    await page.screenshot({
      path: info.outputPath(`composer-feedback-${width}.png`),
      fullPage: true,
      animations: 'disabled',
    });
  }
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  expect(await actions.evaluate((element) => getComputedStyle(element).transitionDuration)).toBe(
    '0s',
  );
  await page.reload();
  await expect(reply.getByRole('button', { name: 'Downvote message' })).toHaveAttribute(
    'aria-pressed',
    'true',
  );
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});

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

test('LOCAL composer settings apply only after a confirmed runtime response and replay an uncertain command', async ({
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
  const details = page.getByRole('form', { name: 'Message Agent' });
  const chip = details.getByRole('button', { name: /^Model and reasoning effort/ });
  await expect(chip).toHaveText('GPT-6 High');
  await chip.click();
  const card = page.getByRole('dialog', { name: 'Model and reasoning effort' });
  const effort = card.getByRole('slider', { name: 'Reasoning effort' });
  await expect(effort).toBeFocused();
  await effort.press('ArrowLeft');
  await expect(effort).toHaveAttribute('aria-valuetext', 'Medium');
  await effort.press('Escape');
  await expect(card).toHaveCount(0);
  await expect(chip).toHaveText('GPT-6 Medium');
  await details.getByRole('button', { name: 'Permission mode', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Guarded', exact: true }).click();
  await details.getByRole('button', { name: 'Apply settings', exact: true }).click();
  await expect(details.getByRole('button', { name: 'Retry settings update' })).toBeEnabled();
  await expect(
    details.getByRole('button', { name: 'Permission mode', exact: true }),
  ).toBeDisabled();
  await chip.click();
  await expect(card.getByRole('button', { name: 'Model', exact: true })).toBeDisabled();
  await expect(effort).toHaveAttribute('aria-disabled', 'true');
  await effort.press('Escape');
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

test('LOCAL composer and Details use model and skill catalogs in bounded responsive containers and confirms stop progress', async ({
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
  await page.locator('.agent-header__strip').hover();
  await page.getByRole('button', { name: 'More Agent actions' }).click();
  await page.getByRole('menuitem', { name: 'Show Agent details' }).click();
  const details = page.getByRole('complementary', { name: 'Agent details' });
  const composer = page.getByRole('form', { name: 'Message Agent' });
  await expect(details.getByRole('button', { name: 'Model', exact: true })).toHaveCount(0);
  const chip = composer.getByRole('button', { name: /^Model and reasoning effort/ });
  await expect(chip).toHaveText('GPT-6 High');
  await chip.click();
  const card = page.getByRole('dialog', { name: 'Model and reasoning effort' });
  await card.getByRole('button', { name: 'Model', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Fast model', exact: true }).click();
  await expect(card.getByRole('button', { name: 'Model', exact: true })).toHaveText('Fast model');
  await expect(chip).toHaveText('Fast model High');
  await page.keyboard.press('Escape');
  await expect(card).toHaveCount(0);
  await expect(details.getByRole('list', { name: 'Services' }).getByRole('listitem')).toHaveCount(
    108,
  );
  await expect(
    details.getByRole('list', { name: 'Available Ornn skills' }).getByRole('listitem'),
  ).toHaveCount(20);
  for (const width of [1440, 390, 320]) {
    await page.setViewportSize({ width, height: 1000 });
    await expect(details).toBeVisible();
    await details.getByRole('heading', { name: 'Details', exact: true }).scrollIntoViewIfNeeded();
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
