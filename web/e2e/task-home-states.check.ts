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

async function expectMobileToolbarFits(page: Page) {
  const footer = page.locator('.task-home__footer');
  await footer.scrollIntoViewIfNeeded();
  const controls = footer.getByRole('button');
  expect(await controls.count()).toBeGreaterThanOrEqual(3);
  for (const control of await controls.all()) {
    await expect(control).toBeVisible();
    await control.scrollIntoViewIfNeeded();
    await expect(control).toBeInViewport({ ratio: 1 });
    await control.click({ trial: true });
  }
  const geometry = await footer.evaluate((node) => {
    const rect = (element: Element) => {
      const { left, right, top, bottom, width, height } = element.getBoundingClientRect();
      return { left, right, top, bottom, width, height };
    };
    return {
      footer: rect(node),
      composer: rect(node.closest('.task-home__composer')!),
      buttons: [...node.querySelectorAll('button')].map((button) => ({
        name: button.getAttribute('aria-label') ?? button.textContent ?? 'Toolbar control',
        container: rect(button.closest('.task-home__repo, .task-home__toolbar') ?? node),
        ...rect(button),
      })),
    };
  });
  for (const [index, button] of geometry.buttons.entries()) {
    expect(button.width, button.name).toBeGreaterThanOrEqual(44);
    expect(button.height, button.name).toBeGreaterThanOrEqual(44);
    for (const container of [button.container, geometry.footer, geometry.composer]) {
      expect(button.left, button.name).toBeGreaterThanOrEqual(container.left);
      expect(button.right, button.name).toBeLessThanOrEqual(container.right);
      expect(button.top, button.name).toBeGreaterThanOrEqual(container.top);
      expect(button.bottom, button.name).toBeLessThanOrEqual(container.bottom);
    }
    for (const other of geometry.buttons.slice(index + 1)) {
      expect(
        button.right <= other.left ||
          other.right <= button.left ||
          button.bottom <= other.top ||
          other.bottom <= button.top,
        `${button.name} must not overlap ${other.name}`,
      ).toBe(true);
    }
  }
  expect(
    await page
      .locator('.task-home__toolbar')
      .evaluate((node) => node.scrollWidth <= node.clientWidth),
  ).toBe(true);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  return geometry;
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
      if (width > 600) {
        expect((await toolbar.getByRole('button').nth(0).boundingBox())!.y).toBe(
          (await toolbar.getByRole('button').nth(1).boundingBox())!.y,
        );
      }
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
        expect((await toolbar.getByRole('button').nth(0).boundingBox())!.y).toBe(
          (await toolbar.getByRole('button').nth(1).boundingBox())!.y,
        );
      }
      await capture('repository');
      await page.getByLabel('Your task').fill('Research the project and summarize the findings.');
      await page.getByRole('button', { name: 'Start task' }).click();
      await expect(page.getByRole('region', { name: 'Original request' })).toBeVisible();
      await expect(
        page.getByRole('heading', { name: 'Getting its cloud computer ready' }),
      ).toBeVisible();
      const details = page.locator('.preparation__details');
      const summary = details.locator('summary');
      const progress = page.getByRole('region', {
        name: 'Agent provisioning progress',
        includeHidden: true,
      });
      await expect(summary).toHaveText('Setup details');
      await expect(details).toHaveJSProperty('open', false);
      await expect(progress).toHaveCount(1);
      await expect(progress).toBeHidden();
      await showFeedback();
      await capture('submitted-preparing');
      await summary.focus();
      await summary.press('Enter');
      await expect(details).toHaveJSProperty('open', true);
      await expect(progress).toBeVisible();
      await showFeedback();
      await capture('submitted-preparing-details');
      await summary.press('Enter');
      await expect(details).toHaveJSProperty('open', false);
      await expect(progress).toBeHidden();
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

for (const theme of ['light', 'dark'] as const) {
  for (const width of [320, 360, 390]) {
    test(`LOCAL task toolbar fits ${width}px with and without a repository${theme === 'dark' ? ' dark' : ''}`, async ({
      page,
      baseURL,
    }, info) => {
      await page.setViewportSize({ width, height: 844 });
      await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
      await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
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
      await page.goto('/');
      await settle(page, width);
      const cloud = page.getByRole('button', { name: /^Cloud computer/ });
      const add = page.getByRole('button', { name: 'Add git repo' });
      const task = page.getByLabel('Your task');
      const start = page.getByRole('button', { name: 'Start task' });
      const draft = 'Keep this draft while choosing a repository.';
      await task.fill(draft);
      await expect(add).toBeEnabled();
      await expect(page.locator('.task-home__divider')).toBeVisible();
      const fits = async (state: string) => {
        const geometry = await expectMobileToolbarFits(page);
        await info.attach(`${state}-geometry`, {
          body: JSON.stringify(geometry, null, 2),
          contentType: 'application/json',
        });
        await page.screenshot({ path: info.outputPath(`${theme}-${width}-${state}.png`) });
        await expect(task).toHaveValue(draft);
        await expect(start).toBeEnabled();
      };
      await fits('without-repository');
      const templates = page.getByRole('button', { name: 'Templates', exact: true });
      await task.focus();
      await page.keyboard.press('Tab');
      await expect(templates).toBeFocused();
      await page.keyboard.press('Tab');
      await expect(cloud).toBeFocused();
      await expect(cloud).not.toHaveCSS('box-shadow', 'none');
      await page.keyboard.press('Shift+Tab');
      await expect(templates).toBeFocused();
      await page.keyboard.press('Shift+Tab');
      await expect(task).toBeFocused();
      await page.keyboard.press('Tab');
      await page.keyboard.press('Tab');
      await cloud.press('Enter');
      await expect(page.getByRole('menu', { name: 'Run this task on' })).toBeVisible();
      await page.keyboard.press('Escape');
      await expect(cloud).toBeFocused();
      await page.keyboard.press('Tab');
      await expect(add).toBeFocused();
      await expect(add).not.toHaveCSS('box-shadow', 'none');
      await page.keyboard.press('Tab');
      await expect(start).toBeFocused();
      await expect(start).toBeInViewport({ ratio: 1 });
      await expect(start).not.toHaveCSS('box-shadow', 'none');
      await page.keyboard.press('Shift+Tab');
      await expect(add).toBeFocused();
      await add.press('Enter');
      const dialog = page.getByRole('dialog', { name: 'Add git repo' });
      await dialog.getByLabel('GitHub repository').fill('owner/repository-with-a-long-name');
      await dialog.getByLabel('Repository ref').fill('feature/a-long-branch-name');
      await dialog.getByLabel('Repository ref').press('Enter');
      await expect(dialog).toHaveCount(0);
      await expect(page.locator('.task-home__divider')).toBeHidden();
      await expect(
        page.getByRole('button', { name: 'owner/repository-with-a-long-name' }),
      ).toBeFocused();
      await fits('with-repository');
      await page.keyboard.press('Tab');
      const remove = page.getByRole('button', { name: 'Remove repository' });
      await expect(remove).toBeFocused();
      await expect(remove).not.toHaveCSS('box-shadow', 'none');
      await page.keyboard.press('Tab');
      await expect(start).toBeFocused();
      await page.keyboard.press('Shift+Tab');
      await expect(remove).toBeFocused();
      await remove.press('Enter');
      await expect(add).toBeFocused();
      await expect(page.locator('.task-home__divider')).toBeVisible();
      await fits('removed-repository');
      expect(boundary.writes).toEqual([]);
      expect(boundary.errors).toEqual([]);
    });
  }
}
