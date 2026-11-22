import { expect, test } from '@playwright/test';
import { newAgentRequestFixture } from './new-agent-request.fixture';

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
    await expect(page.getByRole('region', { name: 'Submitted task' })).toHaveCount(0);
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
    await expect(page.getByRole('region', { name: 'Submitted task' })).toContainText('hello');
    await page.reload();
    await expect(page.getByRole('region', { name: 'Submitted task' })).toContainText('hello');
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
