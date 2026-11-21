import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { insideViewport, noEffects } from './resource-lifecycle.fixture';

for (const width of [1440, 390]) {
  test(`LOCAL named sandbox sections and technical details at ${width}px`, async ({
    page,
    baseURL,
  }, info) => {
    await page.setViewportSize({ width, height: 900 });
    const boundary = new LocalStartup();
    await boundary.route(page, baseURL!);
    await page.route('**/api/v1/agents/agt_contract/skills', (route) => {
      expect(route.request().method()).toBe('GET');
      return route.fulfill({
        json: {
          items: [
            {
              id: 'skill-fixture',
              name: 'TypeScript',
              source: 'ornn',
              version: '1.2',
              status: 'installed',
            },
          ],
          pending: false,
          can_install: true,
        },
      });
    });
    await page.route('**/api/v1/profile-authoring/skills*', (route) => {
      expect(route.request().method()).toBe('GET');
      return route.fulfill({
        json: { items: [{ id: 'skill-fixture', name: 'TypeScript', versions: ['1.2'] }] },
      });
    });
    await page.goto(`/sandboxes/${boundary.sandbox.id}`);
    const main = page.locator('.sandbox-page');
    await expect(main.getByRole('tab', { name: 'Overview', exact: true })).toHaveAttribute(
      'aria-selected',
      'true',
    );
    await expect(main.getByText('Model access', { exact: true })).toBeVisible();
    await expect(main.getByText('Chrono LLM (public)', { exact: true })).toBeVisible();
    await main.getByRole('tab', { name: 'Workspaces', exact: true }).click();
    const row = main
      .getByRole('row')
      .filter({ has: page.locator(`a[href="/workspaces/${boundary.workspace.id}"]`) });
    await expect(row.getByText('Guarded', { exact: true })).toBeVisible();
    await expect(row.getByText('gpt-proof', { exact: true })).toBeVisible();
    await expect(row.locator(`a[href="/agents/${boundary.agent.id}"]`)).toBeVisible();
    await row
      .getByRole('button', { name: 'New agent in project', exact: true })
      .click({ trial: true });
    await main.getByRole('tab', { name: 'Access', exact: true }).click();
    await expect(main.getByRole('link', { name: 'GitHub', exact: true })).toHaveAttribute(
      'href',
      'https://nyxid.ai/services/catalog-github',
    );
    await expect(main.getByText('Workspace limit', { exact: true })).toHaveCount(0);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
      true,
    );
    await page.screenshot({
      path: info.outputPath(`sandbox-sections-${width}.png`),
      fullPage: true,
    });
    const more = page.getByRole('button', { name: 'More actions', exact: true });
    await more.click();
    await page.getByRole('menuitem', { name: 'Technical details', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Technical details', exact: true });
    await expect(dialog.getByText('Workspace limit', { exact: true })).toBeVisible();
    await insideViewport(dialog);
    for (const dark of [false, true]) {
      await page.evaluate(
        (value) => document.body.toggleAttribute('data-ds-dark-theme', value),
        dark,
      );
      await dialog.getByRole('button', { name: 'Close', exact: true }).click({ trial: true });
      await page.screenshot({
        path: info.outputPath(`technical-${width}-${dark}.png`),
        fullPage: true,
      });
    }
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await page.keyboard.press('Escape');
    await expect(dialog).toHaveCount(0);
    await expect(more).toBeFocused();
    await page.goto(`/agents/${boundary.agent.id}`);
    await page.getByRole('button', { name: 'Show Agent details', exact: true }).click();
    const skills = page.getByRole('region', { name: 'Agent skills', exact: true });
    await expect(skills.getByText('TypeScript', { exact: true })).toBeVisible();
    await expect(skills.getByRole('list', { name: 'Agent skill status' })).toContainText(
      '1.2 · Installed',
    );
    await page.getByRole('button', { name: 'Close Agent details', exact: true }).click();
    noEffects(boundary);
  });
}
