import { expect, test } from '@playwright/test';
import { newAgentRequestFixture } from './new-agent-request.fixture';

for (const initial of ['legacy-task', 'fresh'] as const) {
  for (const text of ['hello', 'A different task']) {
    test(`LOCAL ${initial} completion preserves a newly edited unsubmitted ${text} draft`, async ({
      page,
      baseURL,
    }) => {
      await page.setViewportSize({ width: 1440, height: 1100 });
      const local = await newAgentRequestFixture(page, baseURL!, 'provisioning', initial);
      const { fixture, creates } = local;
      const task = page.getByLabel('Your task');
      const start = page.getByRole('button', { name: 'Start task', exact: true });
      const newAgent = page
        .getByRole('complementary', { name: 'Application navigation' })
        .getByRole('button', { name: 'New agent', exact: true });
      if (initial === 'fresh') {
        await page.goto('/agent-manager');
        await task.fill('hello');
        await start.click();
        await expect(page).toHaveURL(new RegExp(`/agents/${fixture.agent.id}$`));
        expect(creates).toHaveLength(1);
        expect(creates[0]!.body).toEqual({ name: 'New task', initial_message: 'hello' });
      } else await page.goto(`/agents/${fixture.agent.id}`);
      await expect(
        page.getByRole('heading', { name: 'Getting its cloud computer ready' }),
      ).toBeVisible();
      await newAgent.click();
      await task.fill('An intervening edit');
      await task.fill(text);
      await expect(start).toBeEnabled();
      const expectedCreates = initial === 'fresh' ? 1 : 0;
      expect(creates).toHaveLength(expectedCreates);
      local.complete();
      await page.goBack();
      await expect(page).toHaveURL(new RegExp(`/agents/${fixture.agent.id}$`));
      await expect
        .poll(() =>
          page.evaluate(
            () =>
              Object.keys(sessionStorage).filter((key) => key.startsWith('cma:agent-recovery:'))
                .length,
          ),
        )
        .toBe(0);
      await newAgent.click();
      await expect(task).toHaveValue(text);
      await expect(start).toBeEnabled();
      await page.reload();
      await expect(task).toHaveValue(text);
      await expect(start).toBeEnabled();
      expect(creates).toHaveLength(expectedCreates);
      expect(fixture.writes).toEqual([]);
      expect(fixture.errors).toEqual([]);
      expect(fixture.unexpected).toEqual([]);
    });
  }
}

test('LOCAL successful creation still clears its unchanged submitted draft', async ({
  page,
  baseURL,
}) => {
  const local = await newAgentRequestFixture(page, baseURL!, 'provisioning', 'fresh');
  await page.goto('/agent-manager');
  await page.getByLabel('Your task').fill('hello');
  await page.getByRole('button', { name: 'Start task', exact: true }).click();
  await expect(page).toHaveURL(new RegExp(`/agents/${local.fixture.agent.id}$`));
  local.complete();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          Object.keys(sessionStorage).filter((key) => key.startsWith('cma:agent-recovery:')).length,
      ),
    )
    .toBe(0);
  await page
    .getByRole('complementary', { name: 'Application navigation' })
    .getByRole('button', { name: 'New agent', exact: true })
    .click();
  await expect(page.getByLabel('Your task')).toHaveValue('');
  await expect(page.getByRole('button', { name: 'Start task', exact: true })).toBeDisabled();
  expect(local.creates).toHaveLength(1);
  expect(local.fixture.writes).toEqual([]);
  expect(local.fixture.errors).toEqual([]);
  expect(local.fixture.unexpected).toEqual([]);
});
