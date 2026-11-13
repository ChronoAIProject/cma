import { test, expect, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

async function settle(page: Page, width: number) {
  await expect
    .poll(() =>
      page.locator('.frame__center').evaluate((node) => node.getBoundingClientRect().right),
    )
    .toBe(width);
  await page.evaluate(() => document.fonts.ready);
}

for (const theme of ['light', 'dark']) {
  for (const width of [1440, 390]) {
    test(`LOCAL task home states ${theme} ${width}`, async ({ page, baseURL }, info) => {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 900 });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.addInitScript(
        (appearance) => localStorage.setItem('cma:appearance', appearance),
        theme,
      );
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
              desktop_enable: true,
              suggested_followup_admission: false,
              browser_takeover: false,
              replay_presentation: false,
            },
          },
        }),
      );
      let outcome: 'running' | 'failed' | 'uncertain' = 'running';
      let writes = 0;
      await page.route('**/api/agent-manager/v1/agents', (route) => {
        writes += 1;
        return outcome === 'uncertain'
          ? route.abort('failed')
          : route.fulfill({ status: 202, json: { id: 'task-home-operation', status: 'queued' } });
      });
      await page.route('**/api/agent-manager/v1/agent-lifecycle/task-home-operation', (route) =>
        route.fulfill({
          json: {
            id: 'task-home-operation',
            status: outcome,
            phase: 'sandbox_ready',
            ...(outcome === 'failed'
              ? { error: 'The cloud computer could not be prepared. Try another task.' }
              : {}),
            events: [],
          },
        }),
      );
      const capture = async (state: string) => {
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        );
        await page.screenshot({
          path: info.outputPath(`${theme}-${width}-${state}.png`),
          animations: 'disabled',
        });
      };
      const showFeedback = async () => {
        const surface = page.locator('.console__surface');
        await surface.evaluate((node) => {
          node.scrollTop = node.scrollHeight;
        });
        await expect
          .poll(() =>
            surface.evaluate((node) =>
              Math.abs(node.scrollHeight - node.clientHeight - node.scrollTop),
            ),
          )
          .toBeLessThanOrEqual(1);
      };
      const reset = async () => {
        await page.evaluate(() => sessionStorage.clear());
        await page.goto('/');
        await settle(page, width);
        await expect(page.getByRole('button', { name: 'Add git repo' })).toBeEnabled();
      };
      await page.goto('/');
      await settle(page, width);
      await expect(page.getByRole('button', { name: 'Add git repo' })).toBeEnabled();
      await capture('empty');
      await page.getByRole('button', { name: /^Cloud computer/ }).click();
      await expect(
        page.getByRole('menuitemradio', { name: 'Cloud computer' }),
      ).toHaveAccessibleDescription('Its own Linux machine in the cloud');
      await capture('menu');
      await page.keyboard.press('Escape');
      const toolbar = page.locator('.task-home__toolbar');
      const before = await toolbar.boundingBox();
      await page.getByRole('button', { name: 'Add git repo' }).click();
      const repository = page.getByRole('dialog', { name: 'Add git repo' });
      const repo = repository.getByLabel('GitHub repository');
      await repo.fill('not-a-repository');
      await repo.press('Enter');
      await expect(repo).toHaveAttribute('aria-invalid', 'true');
      await expect(repo).toHaveAccessibleDescription(
        'Enter owner/repository or a github.com repository URL.',
      );
      await capture('dialog-error');
      await repo.fill('owner/project.git/');
      await repository.getByLabel('Repository ref').fill('main');
      await repository.getByRole('button', { name: 'Add repo', exact: true }).click();
      await expect(repository).toHaveCount(0);
      if (width > 600) {
        expect((await toolbar.boundingBox())!.height).toBe(before!.height);
        expect((await toolbar.boundingBox())!.y).toBe(before!.y);
      }
      await capture('repository');
      await page.getByLabel('Your task').fill('Research the project and summarize the findings.');
      await page.getByRole('button', { name: 'Start task' }).click();
      await expect(page.getByRole('region', { name: 'Submitted task' })).toBeVisible();
      await expect(page.getByText('Preparing a cloud computer for your Agent…')).toBeVisible();
      await expect(page.getByRole('region', { name: 'Agent provisioning progress' })).toBeVisible();
      await showFeedback();
      await capture('submitted-preparing');
      expect(writes).toBe(1);
      outcome = 'uncertain';
      await reset();
      await page.getByLabel('Your task').fill('Keep this exact task if the connection is lost.');
      await page.getByRole('button', { name: 'Start task' }).click();
      await expect(page.getByRole('button', { name: 'Recover original task' })).toBeEnabled();
      await showFeedback();
      await expect(page.getByRole('alert')).toHaveText(
        "We couldn't confirm your task was received. Use Recover original task to resend the same request.",
      );
      await capture('uncertain');
      outcome = 'failed';
      await reset();
      await page.getByLabel('Your task').fill('Prepare a project summary.');
      await page.getByRole('button', { name: 'Start task' }).click();
      await expect(page.getByRole('button', { name: 'Start another task' })).toBeEnabled();
      await showFeedback();
      await expect(
        page.getByText('Task progress and results appear in the conversation.'),
      ).toHaveCount(0);
      await capture('failed');
      await reset();
      await page.getByRole('button', { name: 'Research a topic' }).click();
      // Form-level validation still guards direct submission of a blank task.
      await page.getByLabel('Your task').fill('   ');
      await page
        .getByLabel('Your task')
        .evaluate((node: HTMLTextAreaElement) => node.form!.requestSubmit());
      await expect(page.getByRole('alert')).toHaveText('Describe a task before starting.');
      await showFeedback();
      await capture('validation');
      await page.evaluate(() => sessionStorage.clear());
      await page.goto('/sandboxes/sbx_route');
      await page
        .locator('.sandbox-page')
        .getByRole('button', { name: 'More actions', exact: true })
        .click();
      await page.getByRole('menuitem', { name: 'New agent', exact: true }).click();
      const contextual = page.getByRole('dialog', { name: 'New task', exact: true });
      await expect(contextual.getByText('Creating in Sandbox sbx_route')).toBeVisible();
      await expect(
        contextual.getByRole('region', { name: 'New task in Sandbox sbx_route' }),
      ).toBeVisible();
      await expect(
        contextual.getByRole('heading', { name: 'What should your Agent work on?' }),
      ).toHaveCount(0);
      await expect(contextual.getByRole('button', { name: 'Add git repo' })).toBeDisabled();
      const geometry = await contextual.getByLabel('Your task').evaluate((node) => {
        const input = node.getBoundingClientRect();
        const modal = node.closest('[role="dialog"]')!.getBoundingClientRect();
        const panel = node.closest('.task-home__panel')!.getBoundingClientRect();
        return {
          visible:
            input.top >= modal.top && panel.bottom <= modal.bottom && panel.bottom <= innerHeight,
          scroll: node.closest('.modal__body')!.scrollTop,
        };
      });
      expect(geometry).toEqual({ visible: true, scroll: 0 });
      await contextual.getByRole('button', { name: /^Cloud computer/ }).click();
      await expect(
        page.getByRole('menuitemradio', { name: 'Cloud computer' }),
      ).toHaveAccessibleDescription("This Sandbox's existing Linux machine");
      await page.keyboard.press('Escape');
      await capture('contextual');
      expect(writes).toBe(3);
      expect(boundary.writes).toEqual([]);
    });
  }
}

for (const width of [320, 360, 390]) {
  test(`LOCAL task toolbar fits ${width}px with and without a repository`, async ({
    page,
    baseURL,
  }) => {
    await page.setViewportSize({ width, height: 844 });
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    await page.goto('/');
    await settle(page, width);
    const cloud = page.getByRole('button', { name: /^Cloud computer/ });
    const add = page.getByRole('button', { name: 'Add git repo' });
    await expect(add).toBeEnabled();
    await expect(page.locator('.task-home__divider')).toBeVisible();
    if (width === 390) expect((await cloud.boundingBox())!.y).toBe((await add.boundingBox())!.y);
    const fits = async () => {
      expect(
        await page
          .locator('.task-home__toolbar')
          .evaluate((node) => node.scrollWidth <= node.clientWidth),
      ).toBe(true);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      );
    };
    await fits();
    await add.click();
    const dialog = page.getByRole('dialog', { name: 'Add git repo' });
    await dialog.getByLabel('GitHub repository').fill('owner/repository-with-a-long-name');
    await dialog.getByLabel('Repository ref').fill('feature/a-long-branch-name');
    await dialog.getByRole('button', { name: 'Add repo', exact: true }).click();
    await expect(dialog).toHaveCount(0);
    await expect(page.locator('.task-home__divider')).toBeHidden();
    await fits();
    await page.getByRole('button', { name: 'Remove repository' }).click();
    await expect(page.locator('.task-home__divider')).toBeVisible();
    if (width === 390) expect((await cloud.boundingBox())!.y).toBe((await add.boundingBox())!.y);
    await fits();
    expect(boundary.writes).toEqual([]);
  });
}
