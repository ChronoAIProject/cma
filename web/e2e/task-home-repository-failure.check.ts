import { expect, test } from '@playwright/test';
import {
  RepositoryTaskFixture,
  repository,
  retainedTask,
  submitRepository,
  taskText,
} from './task-home-repository-failure.fixture';

for (const width of [1440, 390]) {
  test(`LOCAL repository failure check preserves the task at ${width}px`, async ({
    page,
    baseURL,
  }, info) => {
    await page.setViewportSize({ width, height: width === 390 ? 844 : 900 });
    const fixture = new RepositoryTaskFixture();
    await fixture.route(page, baseURL!);
    await submitRepository(page);
    const help = page.getByRole('region', { name: 'Repository preparation help' });
    const check = help.getByRole('button', { name: 'Check repository connection' });
    await expect(check).toBeEnabled();
    const original = await retainedTask(page);
    expect(fixture.submissions).toHaveLength(1);
    expect(fixture.submissions[0]?.body).toMatchObject({ initial_message: taskText, repository });
    expect(fixture.dispatched.size).toBe(0);
    expect(fixture.checks).toEqual([]);
    await check.click();
    await expect(help.getByText(/Connections confirmed. The original repository/)).toBeVisible();
    expect(fixture.checks).toEqual([{ target: { type: 'agent', id: fixture.agent.id } }]);
    expect(await retainedTask(page)).toBe(original);
    await help.scrollIntoViewIfNeeded();
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
      true,
    );
    await page.screenshot({ path: info.outputPath(`repository-failed-${width}.png`) });
    await page.reload();
    await expect(check).toBeEnabled();
    expect(fixture.checks).toHaveLength(1);
    expect(await retainedTask(page)).toBe(original);
    fixture.connection = 'missing';
    await check.click();
    const dialog = page.getByRole('dialog', { name: 'Connect your accounts' });
    await expect(dialog).toBeVisible();
    await expect(
      dialog.getByText(/this check does not retry repository preparation/),
    ).toBeVisible();
    fixture.connection = 'satisfied';
    await expect(dialog).toHaveCount(0);
    await expect(help.getByText(/has not been recovered/)).toBeVisible();
    expect(await retainedTask(page)).toBe(original);
    await page.goto(`/agents/${fixture.agent.id}`);
    // Existing conversations discover only explicitly bound retained operations.
    await page.evaluate((agentId) => {
      const key = `cma:agent-creation:${JSON.stringify([location.origin, 'local-owner'])}`;
      const saved = JSON.parse(sessionStorage.getItem(key)!);
      saved.intent.agentId = agentId;
      sessionStorage.setItem(key, JSON.stringify(saved));
    }, fixture.agent.id);
    await page.reload();
    await expect(check).toBeEnabled();
    await check.click();
    await expect(help.getByText(/has not been recovered/)).toBeVisible();
    expect(fixture.submissions).toHaveLength(1);
    expect(fixture.allocations.size).toBe(1);
    expect(fixture.dispatched.size).toBe(0);
    expect(fixture.writes).toEqual([]);
    expect(fixture.unexpected).toEqual([]);
    expect(fixture.errors).toEqual([]);
  });
}

test('LOCAL non-authorization preparation failure offers neutral help and Unknown remains retryable', async ({
  page,
  baseURL,
}) => {
  const fixture = new RepositoryTaskFixture();
  fixture.failure = 'Repository archive could not be extracted.';
  fixture.connection = 'unknown';
  await fixture.route(page, baseURL!);
  await submitRepository(page);
  const check = page.getByRole('button', { name: 'Check repository connection' });
  await expect(check).toBeEnabled();
  const original = await retainedTask(page);
  await check.click();
  await expect(page.getByText(/connection check could not be confirmed/)).toBeVisible();
  await expect(page.getByRole('dialog')).toHaveCount(0);
  fixture.connection = 'satisfied';
  await check.click();
  await expect(page.getByText(/Connections confirmed. The original repository/)).toBeVisible();
  expect(await retainedTask(page)).toBe(original);
  expect(fixture.submissions).toHaveLength(1);
  expect(fixture.dispatched.size).toBe(0);
  expect(fixture.writes).toEqual([]);
  expect(fixture.errors).toEqual([]);
});

for (const state of ['missing', 'unknown'] as const) {
  test(`LOCAL ${state} preflight preserves explicit Continue and the original request`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new RepositoryTaskFixture();
    fixture.preflight = true;
    fixture.connection = state;
    await fixture.route(page, baseURL!);
    await submitRepository(page);
    const next = page.getByRole('button', { name: 'Continue original task' });
    await expect(next).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Check repository connection' })).toHaveCount(0);
    const original = await retainedTask(page);
    expect(fixture.allocations.size).toBe(0);
    expect(fixture.dispatched.size).toBe(0);
    await page.reload();
    await expect(next).toBeEnabled();
    expect(fixture.submissions).toHaveLength(1);
    expect(fixture.checks).toEqual([]);
    await next.click();
    if (state === 'missing') {
      await expect(page.getByRole('dialog')).toBeVisible();
      await page.getByRole('button', { name: 'Cancel', exact: true }).click();
    } else await expect(page.getByText(/connections could not be verified/)).toBeVisible();
    expect(fixture.submissions).toHaveLength(1);
    expect(await retainedTask(page)).toBe(original);
    fixture.connection = 'satisfied';
    await next.click();
    await expect(page.getByRole('button', { name: 'Check repository connection' })).toBeEnabled();
    expect(fixture.submissions).toHaveLength(2);
    expect(fixture.submissions[1]).toEqual(fixture.submissions[0]);
    expect(fixture.allocations.size).toBe(1);
    expect(fixture.dispatched.size).toBe(0);
    expect(fixture.writes).toEqual([]);
    expect(fixture.errors).toEqual([]);
  });
}

test('LOCAL three independent successful tasks each admit one initial message', async ({
  page,
  baseURL,
}) => {
  const fixture = new RepositoryTaskFixture();
  fixture.outcome = 'succeeded';
  await fixture.route(page, baseURL!);
  for (let i = 1; i <= 3; i++) {
    fixture.operationId = `successful-repository-${i}`;
    await submitRepository(page);
    await expect(page).toHaveURL(new RegExp(`/agents/${fixture.agent.id}$`));
    await expect.poll(() => retainedTask(page)).toBeNull();
    expect(fixture.submissions).toHaveLength(i);
    expect(fixture.dispatched.size).toBe(i);
    expect(fixture.submissions[i - 1]?.body).toMatchObject({
      initial_message: taskText,
      repository,
    });
    await page.reload();
    await expect(page.getByRole('form', { name: 'Message Agent' })).toBeVisible();
    expect(fixture.submissions).toHaveLength(i);
    expect(fixture.dispatched.size).toBe(i);
  }
  expect(new Set(fixture.submissions.map((value) => value.key)).size).toBe(3);
  expect(fixture.writes).toEqual([]);
  expect(fixture.unexpected).toEqual([]);
  expect(fixture.errors).toEqual([]);
});
