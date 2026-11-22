import { expect, type Locator, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

export const lifecycleViewports = [
  { width: 1440, height: 900 },
  { width: 800, height: 600 },
  { width: 390, height: 844 },
  { width: 320, height: 480 },
  { width: 320, height: 240 },
  { width: 900, height: 240 },
];

export function noEffects(boundary: LocalStartup) {
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
}

export async function navigation(page: Page, open: boolean) {
  const state = page.locator('.sidebar__head .sidebar__toggle');
  await expect(state).toHaveAttribute('aria-expanded', /^(true|false)$/);
  if ((await state.getAttribute('aria-expanded')) !== String(open))
    await page
      .getByRole('button', { name: open ? 'Show navigation' : 'Hide navigation', exact: true })
      .click();
  await expect(state).toHaveAttribute('aria-expanded', String(open));
}

export async function environments(page: Page) {
  await navigation(page, true);
  await page.getByRole('button', { name: 'Environments', exact: true }).click();
}

export function resourceRow(page: Page, kind: string, id: string) {
  return page.getByRole('treeitem').and(page.locator(`[data-node-id="${kind}:${id}"]`));
}

export async function expand(row: Locator) {
  if ((await row.getAttribute('aria-expanded')) !== 'true')
    await row.getByRole('button', { name: /^Expand / }).click();
}

export async function insideViewport(locator: Locator) {
  await expect(locator).toBeVisible();
  const box = await locator.evaluate((element) => {
    const rect = element.getBoundingClientRect();
    return {
      left: rect.left,
      right: rect.right,
      top: rect.top,
      bottom: rect.bottom,
      width: innerWidth,
      height: innerHeight,
      overflow: element.scrollWidth > element.clientWidth + 1,
    };
  });
  expect(box.left).toBeGreaterThanOrEqual(0);
  expect(box.top).toBeGreaterThanOrEqual(0);
  expect(box.right).toBeLessThanOrEqual(box.width + 1);
  expect(box.bottom).toBeLessThanOrEqual(box.height + 1);
  expect(box.overflow).toBe(false);
}

export async function taskAdmission(page: Page) {
  await page.route('**/api/v2/ui/task-experience', (route) => {
    expect(route.request().method()).toBe('GET');
    return route.fulfill({
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
    });
  });
}
