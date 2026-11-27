import { expect, test } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';
import { desktopBoundary } from './desktop-rfb.fixture';

for (const width of [390, 1600]) {
  test(`LOCAL policy-denied fullscreen expands the same Desktop at ${width}px`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalAgentV2();
    fixture.boundary.agent = { ...fixture.boundary.agent, desktop: true };
    try {
      await page.setViewportSize({ width, height: 844 });
      await fixture.route(page, baseURL!);
      const stream = await desktopBoundary(page, true, (index) => index !== 0);
      await page.route(`${baseURL}/agents/agt_contract`, async (route) => {
        const response = await route.fetch();
        await route.fulfill({
          response,
          headers: { ...response.headers(), 'permissions-policy': 'fullscreen=()' },
        });
      });
      await page.goto('/agents/agt_contract');
      expect(await page.evaluate(() => document.fullscreenEnabled)).toBe(false);

      const message = page.getByRole('textbox', { name: 'Message', exact: true });
      await message.fill('Keep this draft during fullscreen.');
      await page.getByRole('button', { name: 'Show Agent computer' }).click();
      const computer = page.getByRole('complementary', { name: 'Agent computer' });
      const desktop = computer.getByRole('region', { name: 'Machine desktop' });
      const enter = computer.getByRole('button', { name: 'Full screen', exact: true });
      const exit = computer.getByRole('button', { name: 'Exit expanded view' });
      await expect.poll(() => stream.connections.length).toBe(1);
      await expect(desktop.getByRole('button', { name: 'Take control' })).toBeDisabled();
      await enter.click();
      await expect(exit).toBeVisible();
      await expect(computer.getByText(/Open CMA in a tab that allows full screen/i)).toBeVisible();
      await expect
        .poll(() =>
          computer.evaluate((element) => {
            const rect = element.getBoundingClientRect();
            return rect.width === innerWidth && rect.height === innerHeight;
          }),
        )
        .toBe(true);
      stream.connect();
      const takeControl = desktop.getByRole('button', { name: 'Take control' });
      await expect(takeControl).toBeEnabled();
      await page.keyboard.press('Escape');
      await expect(enter).toBeVisible();
      await expect(desktop).toHaveAttribute('data-controller', 'view');

      await takeControl.click();
      await expect(desktop).toHaveAttribute('data-controller', 'user');
      const canvas = await desktop.locator('canvas').elementHandle();
      await enter.click();
      await expect(exit).toBeVisible();
      await exit.click();
      await expect(desktop).toHaveAttribute('data-controller', 'user');
      expect(
        await desktop.locator('canvas').evaluate((node, original) => node === original, canvas),
      ).toBe(true);
      expect(stream.connections).toHaveLength(1);

      await computer.getByRole('button', { name: 'Reconnect' }).click();
      await expect(takeControl).toBeEnabled();
      expect(stream.connections).toHaveLength(2);
      await enter.click();
      await expect(exit).toBeVisible();
      await exit.click();
      await computer.getByRole('button', { name: 'Close computer' }).click();
      await expect(message).toHaveValue('Keep this draft during fullscreen.');
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.writes).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}
