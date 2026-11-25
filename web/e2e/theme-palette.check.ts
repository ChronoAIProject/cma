import { test, expect, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { expectReadableText } from './themeContrast';
async function settle(page: Page, width: number) {
  await expect
    .poll(() =>
      page.locator('.frame__center').evaluate((node) => node.getBoundingClientRect().right),
    )
    .toBe(width);
  await page.evaluate(() => document.fonts.ready);
}

for (const theme of ['light', 'dark'] as const) {
  for (const width of [1440, 390]) {
    test(`LOCAL theme palette ${theme} at ${width}px preserves canonical account and Usage`, async ({
      page,
      baseURL,
    }, info) => {
      await page.setViewportSize({ width, height: width === 390 ? 844 : 900 });
      await page.emulateMedia({ colorScheme: theme, reducedMotion: 'reduce' });
      await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
      const boundary = new LocalStartup();
      const metrics = new LocalUsageMetrics();
      await boundary.route(page, baseURL!);
      await metrics.route(page);
      await metrics.session(page, ['use_own_sandboxes', 'create_sandbox']);
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
      const task = page.getByRole('textbox', { name: 'Your task', exact: true });
      await task.fill('Research the project and summarize the findings.');
      const send = page.getByRole('button', { name: 'Start task', exact: true });
      await expect(send).toBeEnabled();
      await expect(send).toHaveCSS('background-color', 'rgb(200, 245, 66)');
      await expect(send).toHaveCSS('color', 'rgb(11, 11, 12)');
      await expect(page.locator('.task-home__composer')).toHaveCSS(
        'background-color',
        theme === 'light' ? 'rgb(255, 255, 255)' : 'rgb(20, 20, 22)',
      );
      await expect(page.locator('.console__surface')).toHaveCSS(
        'background-color',
        theme === 'light' ? 'rgb(255, 255, 255)' : 'rgb(11, 11, 12)',
      );
      await expectReadableText(page.locator('.task-home'));
      await page.screenshot({ path: info.outputPath(`landing-${theme}-${width}.png`) });
      if (width === 390)
        await page.getByRole('button', { name: 'Show navigation', exact: true }).click();
      const nav = page.getByRole('complementary', { name: 'Application navigation' });
      await expect(nav.getByRole('link', { name: 'Settings', exact: true })).toHaveCount(0);
      await expect(
        nav.locator('.sidebar__actions').getByRole('button', { name: 'Settings', exact: true }),
      ).toHaveCount(0);
      await expect(nav.getByRole('button', { name: 'Settings', exact: true })).toHaveCount(0);
      await expect(
        nav.locator('.sidebar__account').getByRole('button', { name: /Account menu for/ }),
      ).toBeVisible();
      const filter = nav.getByRole('button', { name: 'Filter', exact: true });
      await filter.click();
      const filters = nav.getByRole('group', { name: 'Navigation filters' });
      await expect(filters.getByRole('button', { name: 'Navigation scope' })).toBeVisible();
      await expect(filters.getByRole('button', { name: 'Agent status filter' })).toBeVisible();
      const mode = nav.getByRole('group', { name: 'Navigation view' });
      const environments = mode.getByRole('button', { name: 'Environments', exact: true });
      const tasks = mode.getByRole('button', { name: 'Tasks', exact: true });
      await environments.focus();
      await expectReadableText(filters);
      await page.screenshot({ path: info.outputPath(`tasks-panel-${theme}-${width}.png`) });
      await page.keyboard.press('Enter');
      await expect(environments).toBeFocused();
      await expect(environments).toHaveAttribute('aria-pressed', 'true');
      await expect(filters).toBeHidden();
      await expect(filter).toHaveAttribute('aria-expanded', 'false');
      await expect(environments).not.toHaveCSS('box-shadow', 'none');
      await page.screenshot({ path: info.outputPath(`environments-${theme}-${width}.png`) });
      await page.keyboard.press('Shift+Tab');
      await expect(tasks).toBeFocused();
      await page.keyboard.press('Enter');
      await expect(tasks).toHaveAttribute('aria-pressed', 'true');
      const account = page.getByRole('button', { name: /Account menu for/ });
      await account.click();
      const menu = page.getByRole('menu', { name: 'Account' });
      const appearances = menu.getByRole('group', { name: 'Appearance', exact: true });
      await expect(appearances.getByRole('menuitemradio')).toHaveCount(3);
      await expect(menu.getByRole('menuitem', { name: 'Settings', exact: true })).toHaveCount(0);
      const selectedAppearance = appearances.getByRole('menuitemradio', {
        name: theme === 'light' ? 'Light' : 'Dark',
        exact: true,
      });
      const selectedSize = menu
        .getByRole('group', { name: 'System text size' })
        .getByRole('menuitemradio', { name: 'Medium', exact: true });
      for (const choice of [selectedAppearance, selectedSize]) {
        await expect(choice).toHaveAttribute('aria-checked', 'true');
        await expect(choice).toHaveCSS(
          'color',
          theme === 'light' ? 'rgb(17, 17, 19)' : 'rgb(244, 244, 245)',
        );
        await expect(choice).toHaveCSS('font-weight', '600');
        await expect(choice).toHaveCSS('text-decoration-line', 'none');
      }
      await expectReadableText(menu);
      await page.screenshot({ path: info.outputPath(`account-${theme}-${width}.png`) });
      // Existing canonical radio behavior, including live System changes, is retained.
      await appearances.getByRole('menuitemradio', { name: 'System', exact: true }).click();
      await page.emulateMedia({
        colorScheme: theme === 'dark' ? 'light' : 'dark',
        reducedMotion: 'reduce',
      });
      await expect
        .poll(() => page.locator('body').getAttribute('data-ds-dark-theme'))
        .toBe(theme === 'dark' ? null : '');
      await selectedAppearance.click();
      await expect
        .poll(() => page.evaluate(() => localStorage.getItem('cma:appearance')))
        .toBe(theme);
      await menu.getByRole('menuitem', { name: 'Usage', exact: true }).click();
      const usage = page.getByRole('dialog', { name: 'Your usage' });
      await expect(
        usage.getByRole('button', { name: 'Research Sandbox', exact: true }),
      ).toBeVisible();
      await expect(usage.getByRole('region', { name: 'Allocated capacity now' })).toBeVisible();
      expect(metrics.creates.every((report) => !report.admin)).toBe(true);
      await expectReadableText(usage);
      await usage.locator('.modal__body').evaluate((node) => {
        node.scrollTop = 0;
      });
      await expect(usage.getByRole('button', { name: 'Close', exact: true })).toBeInViewport();
      await page.screenshot({ path: info.outputPath(`usage-${theme}-${width}.png`) });
      await page.keyboard.press('Escape');
      await expect(usage).not.toBeVisible();
      await expect(account).toBeFocused();
      await expect(task).toHaveValue('Research the project and summarize the findings.');
      // Legacy redirects and their notice are develop's, not an alternative Settings UI.
      await page.goto('/settings/general');
      await expect(page).toHaveURL(/\/usage$/);
      const notice = usage.getByText('Appearance is now in your account menu.', { exact: true });
      await expect(notice).toBeVisible();
      await expect(
        usage.getByRole('button', { name: 'Research Sandbox', exact: true }),
      ).toBeVisible();
      await expectReadableText(usage);
      await page.screenshot({ path: info.outputPath(`appearance-notice-${theme}-${width}.png`) });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      );
      expect(boundary.writes).toEqual([]);
      expect(boundary.errors).toEqual([]);
    });
  }
}
