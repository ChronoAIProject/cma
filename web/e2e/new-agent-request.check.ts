import { expect, test } from '@playwright/test';
import { newAgentRequestFixture } from './new-agent-request.fixture';
import { registerRedesignHomeCases } from './redesign-home.cases';
import { LocalStartup } from './agent-startup-fixtures';
import { noEffects, taskAdmission } from './resource-lifecycle.fixture';

registerRedesignHomeCases();

test('LOCAL operator cutoff after an available capacity snapshot preserves the task without retrying', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  await taskAdmission(page);
  let closed = false;
  await page.route('**/api/v1/sandboxes/capacity', (route) =>
    route.fulfill({
      json: {
        resource: 'sandbox',
        limit: closed ? 0 : 2,
        live: 1,
        available: closed ? 0 : 1,
        counts_by_status: { active: 1 },
        observed_at: new Date().toISOString(),
      },
    }),
  );
  const attempts: { key?: string; body: unknown }[] = [];
  await page.route('**/api/agent-manager/v1/agents', (route) => {
    expect(route.request().method()).toBe('POST');
    attempts.push({
      key: route.request().headers()['idempotency-key'],
      body: route.request().postDataJSON(),
    });
    if (attempts.length === 1) {
      closed = true;
      return route.fulfill({
        status: 429,
        json: { code: 'sandbox_limit_reached', limit: 0, live: 1 },
      });
    }
    return route.fulfill({ status: 202, json: { id: 'cutoff-retry', status: 'queued' } });
  });
  await page.route('**/api/agent-manager/v1/agent-lifecycle/cutoff-retry', (route) =>
    route.fulfill({
      json: { id: 'cutoff-retry', status: 'queued', phase: 'sandbox_runtime', events: [] },
    }),
  );
  const retainedCreation = () =>
    page.evaluate(() =>
      sessionStorage.getItem(
        `cma:agent-creation:${JSON.stringify([location.origin, 'local-owner'])}`,
      ),
    );
  await page.goto('/agent-manager');
  const task = page.getByLabel('Your task');
  const start = page.getByRole('button', { name: 'Start task', exact: true });
  await task.fill('Keep this task after the operator cutoff');
  await expect(start).toBeEnabled();
  await start.click();
  await expect(page.getByText(/The Sandbox environment limit is now reached/)).toBeVisible();
  await expect(start).toBeDisabled();
  await expect(task).toHaveValue('Keep this task after the operator cutoff');
  await expect(page.getByRole('button', { name: 'Recover original task' })).toHaveCount(0);
  expect(await retainedCreation()).toBeNull();
  expect(attempts).toHaveLength(1);
  await page.reload();
  await expect(task).toHaveValue('Keep this task after the operator cutoff');
  await expect(start).toBeDisabled();
  closed = false;
  await page.getByRole('button', { name: 'Refresh Sandbox capacity' }).click();
  await expect(start).toBeEnabled();
  expect(attempts).toHaveLength(1);
  await start.click();
  await expect
    .poll(async () => JSON.parse((await retainedCreation()) ?? 'null')?.intent?.receipt?.id)
    .toBe('cutoff-retry');
  expect(attempts).toHaveLength(2);
  expect(attempts[0]!.key).toBeTruthy();
  expect(attempts[1]!.key).toBeTruthy();
  expect(attempts[1]!.key).not.toBe(attempts[0]!.key);
  expect(attempts[1]!.body).toEqual(attempts[0]!.body);
  noEffects(boundary);
});

for (const status of ['provisioning', 'active'] as const) {
  test(`LOCAL New agent keeps hello independent of a bound ${status} Agent`, async ({
    page,
    baseURL,
  }) => {
    await page.setViewportSize({ width: 1440, height: 1100 });
    await page.emulateMedia({ reducedMotion: 'reduce' });
    const local = await newAgentRequestFixture(page, baseURL!, status);
    const { fixture, creates } = local;
    await page.goto(`/agents/${fixture.agent.id}`);
    await expect(page.getByRole('heading', { name: 'Preparing your Agent' })).toBeVisible();
    local.delay();
    await expect.poll(local.held).toBe(true);
    const newAgent = page
      .getByRole('complementary', { name: 'Application navigation' })
      .getByRole('button', { name: 'New agent', exact: true });
    await newAgent.click();
    await expect(page).toHaveURL(/\/agent-manager$/);
    const task = page.getByLabel('Your task');
    const start = page.getByRole('button', { name: 'Start task', exact: true });
    await task.fill('hello');
    await expect(start).toBeEnabled();
    await expect(page.getByRole('region', { name: 'Original request' })).toHaveCount(0);
    await expect(page.getByText('Preparing a cloud computer for your Agent…')).toHaveCount(0);
    await page.clock.install();
    local.release();
    await page.clock.runFor(30_000);
    await expect(page).toHaveURL(/\/agent-manager$/);
    await expect(task).toHaveValue('hello');
    await expect(start).toBeEnabled();
    await page.clock.resume();
    await newAgent.click();
    await page.goBack();
    await expect(page).toHaveURL(/\/agent-manager$/);
    await page.goBack();
    await expect(page).toHaveURL(new RegExp(`/agents/${fixture.agent.id}$`));
    await expect(page.getByRole('heading', { name: 'Preparing your Agent' })).toBeVisible();
    await page.goForward();
    await expect(page).toHaveURL(/\/agent-manager$/);
    await page.reload();
    await expect(task).toHaveValue('hello');
    await expect(start).toBeEnabled();
    expect(creates).toEqual([]);
    expect(fixture.writes).toEqual([]);
    const journalBefore = await page.evaluate(() => {
      const key = Object.keys(sessionStorage).find((key) => key.startsWith('cma:agent-recovery:'));
      return key ? sessionStorage.getItem(key) : null;
    });
    expect(JSON.parse(journalBefore!).intent).toMatchObject({
      key: 'retained-old-request',
      agentId: fixture.agent.id,
      receipt: { id: 'old-operation' },
    });
    const accepted = page.waitForResponse(
      (response) =>
        response.request().method() === 'POST' &&
        new URL(response.url()).pathname === '/api/agent-manager/v1/agents',
    );
    await start.click();
    expect((await accepted).status()).toBe(202);
    await expect.poll(() => creates.length).toBe(1);
    expect(creates[0]!.body).toEqual({ name: 'New task', initial_message: 'hello' });
    expect(creates[0]!.key).toBeTruthy();
    expect(creates[0]!.key).not.toBe('retained-old-request');
    const retainedCreation = () =>
      page.evaluate(() => {
        const key = `cma:agent-creation:${JSON.stringify([location.origin, 'local-owner'])}`;
        return sessionStorage.getItem(key);
      });
    await expect
      .poll(async () => JSON.parse((await retainedCreation()) ?? 'null')?.intent?.receipt?.id)
      .toBe('new-operation');
    const creationBeforeReload = await retainedCreation();
    expect(JSON.parse(creationBeforeReload!).intent).toMatchObject({
      key: creates[0]!.key,
      body: creates[0]!.body,
      receipt: { id: 'new-operation', status: 'queued' },
    });
    await expect(page.getByRole('region', { name: 'Original request' })).toContainText('hello');
    await page.reload();
    await expect(page.getByRole('region', { name: 'Original request' })).toContainText('hello');
    await expect(page).toHaveURL(/\/agent-manager$/);
    expect(creates).toHaveLength(1);
    expect(await retainedCreation()).toBe(creationBeforeReload);
    const journalAfter = await page.evaluate(() => {
      const key = Object.keys(sessionStorage).find((key) => key.startsWith('cma:agent-recovery:'));
      return key ? sessionStorage.getItem(key) : null;
    });
    expect(journalAfter).toBe(journalBefore);
    expect(fixture.writes).toEqual([]);
    expect(fixture.errors).toEqual([]);
    expect(fixture.unexpected).toEqual([]);
  });
}
