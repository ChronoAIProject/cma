import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

for (const theme of ['light', 'dark']) {
  for (const width of [1440, 390]) {
    test(`LOCAL ${theme} ${width}: failed rotation preserves task outcomes without activity`, async ({
      page,
      baseURL,
    }, info) => {
      const boundary = new LocalStartup();
      boundary.sandbox = {
        ...boundary.sandbox,
        status: 'rotating',
        lifecycle: {
          pause: { allowed: false },
          revive: { allowed: false },
          operation: {
            purpose: 'image_rotation',
            phase: 'conversation_reopen',
            started_at: '2026-09-05T00:00:00Z',
          },
        },
        runtime_update: {
          current_version: 'runtime-before',
          latest_version: 'runtime-after',
          state: 'failed',
          update_available: true,
          can_update: false,
          schedule: { timezone: 'Asia/Singapore', local_time: '02:00' },
          preserves_home: true,
        },
      };
      const original = boundary.workspace.agents[0]!;
      boundary.workspace = {
        ...boundary.workspace,
        agents: [
          { ...original, status: 'ready', task_status: 'completed', title: 'Completed research' },
          {
            ...original,
            id: 'agt_working',
            status: 'working',
            title: 'Accepted investigation',
            task_started_at: '2026-09-05T00:00:00Z',
          },
        ],
      };
      await page.addInitScript(
        (appearance) => localStorage.setItem('cma:appearance', appearance),
        theme,
      );
      await page.setViewportSize({ width, height: 900 });
      await boundary.route(page, baseURL!);
      await page.route('**/api/v1/agents/agt_working/channel-usage', (route) =>
        route.fulfill({
          json: {
            agent_id: 'agt_working',
            channel_usage: {
              active: false,
              has_history: false,
              first_used_at: null,
              last_used_at: null,
            },
          },
        }),
      );
      await page.route('**/api/v1/agents/agt_working/provenance', (route) =>
        route.fulfill({
          json: {
            agent_id: 'agt_working',
            created_at: '2026-09-05T00:00:00Z',
            creation_origin: 'manual',
          },
        }),
      );
      await page.goto('/');
      if (width < 600)
        await page.getByRole('button', { name: 'Show navigation', exact: true }).click();
      const tree = page.getByRole('tree', { name: 'Personal Agents' });
      const group = tree.getByRole('group', { name: 'Needs attention' });
      await expect(group).toBeVisible();
      await expect(
        group.getByRole('treeitem', {
          name: /Completed research, Completed, Runtime update failed/,
        }),
      ).toBeVisible();
      await expect(
        group.getByRole('treeitem', {
          name: /Accepted investigation, Needs attention, Runtime update failed/,
        }),
      ).toBeVisible();
      await expect(tree.getByRole('group', { name: 'Active' })).toHaveCount(0);
      await expect(tree.locator('.task-running')).toHaveCount(0);
      await expect(tree.locator('[data-pose="preparing"]')).toHaveCount(0);
      for (const notice of await tree.locator('.task-environment-notice').all()) {
        await expect(notice).toBeVisible();
        expect(await notice.evaluate((node) => node.scrollWidth <= node.clientWidth + 1)).toBe(
          true,
        );
      }
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
      ).toBe(true);
      await page.screenshot({ path: info.outputPath(`rotation-${theme}-${width}.png`) });
      await group.getByRole('button', { name: 'Options for Completed research' }).click();
      await page.getByRole('menuitem', { name: 'View environment' }).click();
      await expect(page).toHaveURL(/\/sandboxes\/sbx_route$/);
      expect(boundary.writes).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.errors).toEqual([]);
    });
  }
}
