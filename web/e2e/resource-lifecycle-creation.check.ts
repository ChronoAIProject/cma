import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import {
  insideViewport,
  lifecycleViewports,
  navigation,
  noEffects,
  taskAdmission,
} from './resource-lifecycle.fixture';

for (const viewport of lifecycleViewports) {
  test(`LOCAL task page and contextual modal controls remain reachable at ${viewport.width}x${viewport.height}`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    await taskAdmission(page);
    await page.setViewportSize(viewport);
    await page.goto(`/sandboxes/${boundary.sandbox.id}`);
    await navigation(page, true);
    await page
      .getByRole('complementary', { name: 'Application navigation' })
      .getByRole('button', { name: 'New agent', exact: true })
      .click();
    await navigation(page, false);
    await expect(page).toHaveURL(`${baseURL}/agent-manager`);
    await expect(
      page.getByRole('heading', { name: /^Good (morning|afternoon|evening)\.$/ }),
    ).toBeVisible();
    await expect(page.getByRole('dialog')).toHaveCount(0);
    const task = page.getByRole('textbox', { name: 'Your task', exact: true });
    await task.click();
    await task.pressSequentially('Keep this task');
    await task.press('Shift+Enter');
    await task.pressSequentially('and this line');
    await expect(task).toHaveValue('Keep this task\nand this line');
    await page.keyboard.press('Tab');
    await expect(page.getByRole('button', { name: 'Cloud computer', exact: true })).toBeFocused();
    await page.keyboard.press('Tab');
    await expect(page.getByRole('button', { name: 'Add git repo', exact: true })).toBeFocused();
    await page.keyboard.press('Tab');
    const start = page.getByRole('button', { name: 'Start task', exact: true });
    await expect(start).toBeFocused();
    await start.click({ trial: true });
    await insideViewport(start);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
      true,
    );

    // This action has a real parent and a modal; global New agent is a page.
    await page.goto(`/sandboxes/${boundary.sandbox.id}`);
    await page.getByRole('tab', { name: 'Workspaces', exact: true }).click();
    const open = page.getByRole('button', { name: 'New workspace', exact: true });
    await open.click();
    const dialog = page.getByRole('dialog', { name: 'New workspace', exact: true });
    await expect(dialog.locator('.modal__body')).toBeVisible();
    await dialog.getByRole('button', { name: 'Workspace source', exact: true }).click();
    await page
      .getByRole('menuitemradio', { name: 'Clone a GitHub repository', exact: true })
      .click();
    const repository = dialog.getByLabel('GitHub repository URL');
    await repository.fill('https://github.com/example/repository');
    await repository.click();
    await expect(repository).toHaveValue('https://github.com/example/repository');
    await dialog.getByLabel('Workspace name').fill('lifecycle-fixture');
    await dialog.getByLabel('Branch or ref (optional)').fill('main');
    const create = dialog.getByRole('button', { name: 'Create workspace', exact: true });
    await create.click({ trial: true });
    await insideViewport(dialog);
    await insideViewport(create);
    await page.screenshot({
      path: info.outputPath(`modal-${viewport.width}x${viewport.height}-footer.png`),
    });
    await dialog.getByRole('button', { name: 'Close', exact: true }).click();
    await expect(dialog).toHaveCount(0);
    await expect(open).toBeFocused();
    await open.press('Enter');
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(dialog).toHaveCount(0);
    await expect(open).toBeFocused();
    noEffects(boundary);
  });
}
