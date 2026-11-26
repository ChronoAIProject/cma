import { expect, test } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';
import { desktopBoundary } from './desktop-rfb.fixture';

test.use({ locale: 'en-GB', timezoneId: 'America/Argentina/Rio_Gallegos' });

for (const width of [1440, 390]) {
  test(`LOCAL desktop local time remains readable at ${width}px`, async ({ page, baseURL }) => {
    const fixture = new LocalAgentV2();
    fixture.boundary.agent = { ...fixture.boundary.agent, desktop: true };
    try {
      await page.setViewportSize({ width, height: 900 });
      await page.clock.setFixedTime(new Date('2026-11-24T04:09:00Z'));
      await fixture.route(page, baseURL!);
      await desktopBoundary(page);
      await page.goto('/agents/agt_contract');
      await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
      const desktop = page.getByRole('region', { name: 'Machine desktop', exact: true });
      await expect(desktop.getByRole('status')).toContainText('Agent paused · Desktop is idle');
      const hint = desktop.locator('header p', { hasText: 'Local time' });
      await expect(hint).toHaveText('Local time · America/Argentina/Rio_Gallegos · Tue 01:09');
      await expect(hint.locator('time')).toHaveAttribute('datetime', '2026-11-24T04:09:00.000Z');
      await expect(hint).toBeVisible();
      const bounds = await hint.boundingBox();
      expect(bounds).not.toBeNull();
      expect(bounds!.x).toBeGreaterThanOrEqual(0);
      expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
      expect(await hint.evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
      await expect(desktop.getByRole('button', { name: 'Take control' })).toBeEnabled();
      await expect(page.getByRole('button', { name: 'Enable browser', exact: true })).toHaveCount(
        0,
      );
      expect(fixture.writes).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}
