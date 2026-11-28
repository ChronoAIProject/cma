import { expect, test, type Page, type TestInfo } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';
import { navigation, noEffects, taskAdmission } from './resource-lifecycle.fixture';

test('LOCAL the last live environment scrolls fully into view in a short sidebar', async ({
  page,
  baseURL,
}) => {
  await page.setViewportSize({ width: 1440, height: 420 });
  const boundary = new LocalStartup();
  const sandboxes = Array.from({ length: 10 }, (_, index) => ({
    ...boundary.sandbox,
    id: `sbx_scroll_${index + 1}`,
    name: index < 5 ? `Live environment ${index + 1}` : `Retiring environment ${index - 4}`,
    status: index < 5 ? 'active' : 'retiring',
    workspaces: [],
    workspace_count: 0,
    worktree_count: 0,
    agent_count: 0,
  }));
  await boundary.route(page, baseURL!);
  await page.route('**/api/v1/sandboxes?*', (route) =>
    route.fulfill({ json: { items: sandboxes } }),
  );
  for (const sandbox of sandboxes)
    await page.route(`**/api/v1/sandboxes/${sandbox.id}`, (route) =>
      route.fulfill({ json: sandbox }),
    );

  await page.goto('/agent-manager');
  await navigation(page, true);
  const sidebar = page.getByRole('complementary', { name: 'Application navigation' });
  const environments = sidebar.getByRole('button', { name: 'Environments', exact: true });
  await environments.click();
  await sidebar.locator('[data-navigation-footer] summary').click();
  await environments.focus();
  const last = sidebar.getByRole('tree', { name: 'Live sandboxes' }).getByRole('treeitem').last();
  await expect(last).toContainText('Live environment 5');
  await expect(last).not.toBeInViewport({ ratio: 1 });
  const list = (await sidebar.locator('.manager-navigation__list').boundingBox())!;
  const row = (await last.boundingBox())!;
  await page.mouse.move(list.x + 24, list.y + 10);
  await page.mouse.wheel(0, Math.ceil(row.y - list.y));
  await expect(last).toBeInViewport({ ratio: 1 });
  noEffects(boundary);
});

function parked() {
  const boundary = new LocalStartup();
  boundary.sandbox = {
    ...boundary.sandbox,
    status: 'rotating',
    lifecycle: {
      pause: { allowed: false },
      revive: { allowed: false },
      operation: { purpose: 'image_rotation', phase: 'conversation_reopen', started_at: localTime },
      result: {
        state: 'failed',
        code: 'runtime_update_parked',
        continuing: false,
        operation_id: 'original-update',
        started_at: localTime,
        result_due_at: localTime,
        actions: ['observe'],
      },
    },
  };
  return boundary;
}

async function install(page: Page, baseURL: string, boundary: LocalStartup, dark: boolean) {
  await page.emulateMedia({ reducedMotion: 'reduce', colorScheme: dark ? 'dark' : 'light' });
  await page.addInitScript((value) => {
    localStorage.setItem('cma:theme', value ? 'dark' : 'light');
  }, dark);
  await boundary.route(page, baseURL);
  await taskAdmission(page);
  await page.route('**/api/v1/agents/*/reply-preview', (route) => {
    expect(route.request().method()).toBe('GET');
    return route.fulfill({
      status: 404,
      json: { code: 'not_found', detail: 'Optional preview unavailable.' },
    });
  });
}

async function capture(page: Page, info: TestInfo, name: string, dark: boolean) {
  await page.evaluate((value) => document.body.toggleAttribute('data-ds-dark-theme', value), dark);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
  await page.screenshot({
    path: info.outputPath(`${name}-${dark ? 'dark' : 'light'}.png`),
    fullPage: true,
    animations: 'disabled',
  });
}

for (const width of [1440, 390, 320]) {
  for (const dark of [false, true]) {
    const label = `${width}px ${dark ? 'dark' : 'light'}`;
    test(`LOCAL grouped runtime attention is keyboard reachable and navigation only at ${label}`, async ({
      page,
      baseURL,
    }, info) => {
      await page.setViewportSize({ width, height: 900 });
      const boundary = parked();
      const empty = {
        ...boundary.sandbox,
        id: 'sbx_0123456789abcdef0123456789abcdef',
        name: 'Environment without Agents sandbox 0123456789abcdef0123456789abcdef',
        workspaces: [],
        workspace_count: 0,
        worktree_count: 0,
        agent_count: 0,
      };
      await install(page, baseURL!, boundary, dark);
      await page.route('**/api/v1/sandboxes?*', (route) =>
        route.fulfill({ json: { items: [boundary.sandbox, empty] } }),
      );
      await page.route(`**/api/v1/sandboxes/${empty.id}`, (route) =>
        route.fulfill({ json: empty }),
      );
      await page.route(`**/api/v1/sandboxes/${empty.id}/events`, (route) =>
        route.fulfill({ json: [] }),
      );
      await page.route(`**/api/v1/resource-access/sandbox/${empty.id}`, (route) =>
        route.fulfill({
          json: {
            capabilities: ['view_details', 'view_history', 'manage_sharing'],
            owner: { kind: 'person', is_current_person: true },
            revalidate_at: null,
            policy_revision: 1,
          },
        }),
      );
      await page.goto('/agent-manager');
      await navigation(page, true);
      const sidebar = page.getByRole('complementary', { name: 'Application navigation' });
      const disclosure = sidebar.locator('.runtime-attention');
      const summary = disclosure.locator('summary');
      await expect(summary).toHaveText('2 updates failed · Fix');
      await expect(disclosure).not.toHaveAttribute('open');
      await summary.focus();
      await page.keyboard.press('Enter');
      await expect(disclosure).toHaveAttribute('open', '');
      await expect(
        disclosure
          .getByRole('list', { name: 'Environments with failed updates' })
          .locator(':scope > li'),
      ).toHaveCount(2);
      await expect(disclosure.locator('.character-avatar').first()).toHaveAttribute(
        'data-motion',
        'still',
      );
      const name = disclosure.getByRole('button', { name: empty.name, exact: true });
      await expect(disclosure.getByText(empty.id, { exact: true })).toBeVisible();
      expect(
        await name.evaluate((button) => {
          const id = button.parentElement!.querySelector('code')!;
          const textBounds = (element: Element) => {
            const range = document.createRange();
            range.selectNodeContents(element);
            return range.getBoundingClientRect();
          };
          const nameText = textBounds(button);
          const idText = textBounds(id);
          const row = button.parentElement!.getBoundingClientRect();
          return {
            nameFits: nameText.left >= row.left && nameText.right <= row.right,
            idFits: idText.left >= row.left && idText.right <= row.right,
            separate: nameText.bottom <= idText.top,
          };
        }),
      ).toEqual({ nameFits: true, idFits: true, separate: true });
      await capture(page, info, 'runtime-attention', dark);
      await sidebar.getByRole('button', { name: 'Environments', exact: true }).click();
      await sidebar.getByRole('button', { name: 'Filter', exact: true }).click();
      await sidebar.getByRole('button', { name: 'Sandbox status filter' }).click();
      await page.getByRole('menuitemradio', { name: 'Paused', exact: true }).click();
      await sidebar.getByRole('button', { name: 'Tasks', exact: true }).click();
      await navigation(page, false);
      await expect(
        page
          .getByRole('main')
          .getByText('2 sandboxes couldn’t update their runtime', { exact: true }),
      ).toBeVisible();
      await page.getByRole('main').getByRole('button', { name: 'Review', exact: true }).click();
      await expect(sidebar.getByRole('tree', { name: 'Needs attention sandboxes' })).toBeVisible();
      await expect(
        sidebar
          .getByRole('tree', { name: 'Needs attention sandboxes' })
          .locator('[role="treeitem"][aria-level="1"]'),
      ).toHaveCount(2);
      await sidebar.getByRole('button', { name: 'Tasks', exact: true }).click();
      await name.click();
      await expect(page).toHaveURL(`${baseURL}/sandboxes/${empty.id}`);
      noEffects(boundary);
    });

    test(`LOCAL cleanup footer preserves UNKNOWN and capacity while fitting ${label}`, async ({
      page,
      baseURL,
    }, info) => {
      await page.setViewportSize({ width, height: 900 });
      const boundary = new LocalStartup();
      boundary.sandbox = { ...boundary.sandbox, status: 'retiring' };
      await install(page, baseURL!, boundary, dark);
      await page.addInitScript(() => {
        for (const [id, accepted] of [
          ['sbx_route', true],
          ['sbx_unknown', false],
        ] as const) {
          const request = {
            action: { kind: 'delete_sandbox', sandboxId: id },
            key: `local-${id}`,
            accepted,
          };
          localStorage.setItem(
            `cma:deletion-requests:local-owner:${request.key}`,
            JSON.stringify(request),
          );
        }
        Object.defineProperty(navigator, 'clipboard', {
          configurable: true,
          value: { writeText: () => Promise.reject(new Error('Clipboard unavailable')) },
        });
      });
      await page.route('**/api/v1/sandboxes/capacity', (route) =>
        route.fulfill({
          json: {
            resource: 'sandbox',
            limit: 2,
            live: 2,
            available: 0,
            counts_by_status: { retiring: 1, active: 1 },
            observed_at: localTime,
          },
        }),
      );
      await page.route('**/api/v1/sandboxes/sbx_unknown', (route) =>
        route.fulfill({
          status: 503,
          json: { code: 'unavailable', detail: 'Deletion status unavailable.' },
        }),
      );
      await page.goto('/agent-manager');
      await page.getByRole('dialog').getByLabel('Close', { exact: true }).click();
      await navigation(page, true);
      const footer = page.locator('.manager-navigation > .deletion-progress');
      await expect(footer.locator('summary')).toHaveText('2 deletions need checking');
      await footer.locator('summary').focus();
      await page.keyboard.press('Enter');
      await expect(footer.getByText('Deletion not confirmed', { exact: true })).toBeVisible();
      await expect(footer.getByText('Deleting', { exact: true })).toBeVisible();
      await footer
        .getByRole('button', { name: 'Copy identifier for Environment', exact: true })
        .click();
      await expect(
        footer.getByText('Copy unavailable. Select the full identifier below.'),
      ).toBeVisible();
      await expect(footer.locator('.deletion-progress__full')).toHaveText('sbx_unknown');
      expect(
        await footer.evaluate((element) => element.closest('.manager-navigation__list') === null),
      ).toBe(true);
      await capture(page, info, 'cleanup-footer', dark);
      if (width === 320) {
        await page.setViewportSize({ width, height: 480 });
        expect(
          await footer.evaluate((element) => {
            const account = document.querySelector('.sidebar__account')!.getBoundingClientRect();
            return (
              element.getBoundingClientRect().bottom <= account.top && account.bottom <= innerHeight
            );
          }),
        ).toBe(true);
      }
      await navigation(page, false);
      await expect(
        page.getByRole('main').getByText('Environment limit reached', { exact: false }),
      ).toBeVisible();
      noEffects(boundary);
    });

    test(`LOCAL selected retired Agent remains reachable without hierarchy at ${label}`, async ({
      page,
      baseURL,
    }, info) => {
      await page.setViewportSize({ width, height: 900 });
      const boundary = new LocalStartup();
      boundary.sandbox = { ...boundary.sandbox, status: 'retired' };
      boundary.agent = {
        ...boundary.agent,
        owner_title: 'Retained task',
        sandbox: { ...boundary.agent.sandbox, status: 'retired' },
      };
      await install(page, baseURL!, boundary, dark);
      await page.route('**/api/v1/sandboxes?*', (route) => route.fulfill({ json: { items: [] } }));
      await page.goto(`/agents/${boundary.agent.id}`);
      await navigation(page, true);
      const sidebar = page.getByRole('complementary', { name: 'Application navigation' });
      const group = sidebar.getByRole('group', { name: 'Retired', exact: true });
      await expect(group.getByRole('link', { name: 'Retained task, Retired' })).toHaveAttribute(
        'aria-current',
        'page',
      );
      await expect(sidebar.getByRole('treeitem')).toHaveCount(1);
      await expect(group.locator('.agent-mascot__poster')).toHaveAttribute('src', /poster-/);
      await expect(sidebar.getByText('No other agents to show.')).toBeVisible();
      await sidebar.getByRole('button', { name: 'Filter', exact: true }).click();
      await sidebar.getByRole('button', { name: 'Agent status filter' }).click();
      await page.getByRole('menuitemradio', { name: 'Working', exact: true }).click();
      await expect(sidebar.getByText('No agents match these filters.')).toBeVisible();
      await expect(group).toBeVisible();
      await sidebar.getByRole('button', { name: 'Show all', exact: true }).click();
      await expect(sidebar.getByRole('button', { name: 'Agent status filter' })).toContainText(
        'All statuses',
      );
      await capture(page, info, 'retired-navigation', dark);
      await page.reload();
      await navigation(page, true);
      await expect(group).toBeVisible();
      noEffects(boundary);
    });

    test(`LOCAL Home capacity entry opens the mounted Environment navigation at ${label}`, async ({
      page,
      baseURL,
    }, info) => {
      await page.setViewportSize({ width, height: 900 });
      const boundary = new LocalStartup();
      await install(page, baseURL!, boundary, dark);
      await page.route('**/api/v1/sandboxes/capacity', (route) =>
        route.fulfill({
          json: {
            resource: 'sandbox',
            limit: 1,
            live: 1,
            available: 0,
            counts_by_status: { active: 1 },
            observed_at: localTime,
          },
        }),
      );
      await page.goto('/agent-manager');
      await navigation(page, false);
      await page
        .getByRole('main')
        .getByRole('button', { name: 'Environments', exact: true })
        .click();
      const sidebar = page.getByRole('complementary', { name: 'Application navigation' });
      await expect(
        sidebar.getByRole('button', { name: 'Environments', exact: true }),
      ).toHaveAttribute('aria-pressed', 'true');
      await expect(
        sidebar.getByRole('button', { name: 'Environments', exact: true }),
      ).toBeVisible();
      await expect(sidebar.getByRole('button', { name: 'Manage environments' })).toHaveCount(0);
      await capture(page, info, 'home-environments', dark);
      await sidebar.getByRole('button', { name: 'Tasks', exact: true }).click();
      await expect(page).toHaveURL(`${baseURL}/agent-manager`);
      noEffects(boundary);
    });
  }
}
