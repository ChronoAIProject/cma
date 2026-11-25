import { expect, test } from '@playwright/test';
import { homeSimplificationFixture, selectHomeTemplate } from './home-simplification.fixture';

for (const theme of ['dark', 'light']) {
  for (const state of [
    'crew',
    'empty',
    'near',
    'full',
    'capacity-error',
    'navigation-error',
  ] as const) {
    test(`LOCAL simplified Home ${theme} ${state} preserves admission and real reply provenance`, async ({
      page,
      baseURL,
    }, info) => {
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
      const local = await homeSimplificationFixture(page, baseURL!, state);
      await page.goto('/agent-manager');
      const input = page.getByLabel('Your task', { exact: true });
      await expect(input).toBeVisible();
      await expect(page.getByText('Default model', { exact: true })).toBeVisible();
      await expect(page.getByRole('button', { name: 'Default model' })).toHaveCount(0);
      await expect(page.locator('.task-home__intro')).toHaveCount(0);
      if (state === 'empty') {
        await expect(
          page.getByRole('button', { name: 'Research a topic', exact: true }),
        ).toBeVisible();
        await expect(page.locator('.task-home__templates button')).toHaveCount(5);
        await expect(page.getByRole('button', { name: 'Templates', exact: true })).toHaveCount(0);
        expect(local.previews).toEqual([]);
      } else if (state === 'navigation-error') {
        await expect(page.getByRole('main').getByRole('alert')).toBeVisible();
        await expect(page.locator('[data-first-visit="true"]')).toHaveCount(0);
      } else {
        const cards = page.locator('.task-crew__card');
        await expect(cards).toHaveCount(3);
        await expect(cards.locator('.task-crew__description')).toHaveText([
          'A real recorded reply: 中文 <sample> — synthetic findings.',
        ]);
        await expect(cards.locator('time')).toHaveAttribute('datetime', '2026-11-22T05:55:00Z');
        expect(new Set(local.previews).size).toBe(3);
        await expect(page.getByRole('button', { name: 'Templates', exact: true })).toBeVisible();
      }
      if (state === 'near')
        await expect(page.getByText('1 slot left', { exact: true })).toBeVisible();
      if (state === 'full')
        await expect(page.getByText('Environment limit reached', { exact: true })).toBeVisible();
      if (state === 'capacity-error')
        await expect(
          page.getByText('Capacity could not be checked', { exact: true }),
        ).toBeVisible();
      if (state === 'full' || state === 'capacity-error') {
        await expect(page.getByRole('region', { name: 'Sandbox capacity' })).toHaveCount(1);
        await input.fill('Kept until current capacity is confirmed');
        await expect(page.getByRole('button', { name: 'Start task', exact: true })).toBeDisabled();
        await input.press('Control+Enter');
        expect(local.writes).toEqual([]);
      }
      await page.clock.runFor(200);
      await page.evaluate(() => document.fonts.ready);
      await expect(page.locator('.topographic-flow')).toHaveCount(0);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      );
      await page.screenshot({
        path: info.outputPath(`home-${state}-${theme}.png`),
        animations: 'disabled',
      });
      expect(local.writes).toEqual([]);
      expect(local.boundary.writes).toEqual([]);
      expect(local.boundary.errors).toEqual([]);
      expect(local.boundary.unexpected).toEqual([]);
    });
  }
  for (const width of [320, 390, 720]) {
    test(`LOCAL simplified Home ${theme} ${width} template menu preserves keyboard focus and independent draft`, async ({
      page,
      baseURL,
    }, info) => {
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
      const local = await homeSimplificationFixture(page, baseURL!);
      await page.goto('/agent-manager');
      const input = page.getByLabel('Your task', { exact: true });
      await input.fill('Independent draft with [brackets]');
      const menu = page.getByRole('button', { name: 'Templates', exact: true });
      await menu.focus();
      await menu.press('Enter');
      await expect(page.getByRole('menuitem')).toHaveCount(5);
      await page.keyboard.press('End');
      await expect(page.getByRole('menuitem', { name: 'Write code', exact: true })).toBeFocused();
      await page.keyboard.press('Home');
      await expect(
        page.getByRole('menuitem', { name: 'Research a topic', exact: true }),
      ).toBeFocused();
      await page.screenshot({
        path: info.outputPath(`home-menu-${theme}-${width}.png`),
        animations: 'disabled',
      });
      await page.keyboard.press('Escape');
      await expect(menu).toBeFocused();
      await selectHomeTemplate(page, 'Research a topic');
      await expect(page.getByLabel('Topic', { exact: true })).toBeFocused();
      await page.getByRole('button', { name: 'Write a task without a template' }).click();
      await expect(page.getByRole('dialog')).toHaveCount(0);
      await expect(input).toHaveValue('Independent draft with [brackets]');
      await expect(input).toBeFocused();
      expect(local.writes).toEqual([]);
      expect(local.boundary.errors).toEqual([]);
    });
  }
}
