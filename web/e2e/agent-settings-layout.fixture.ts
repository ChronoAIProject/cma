import { expect, type Locator, type Page, type TestInfo } from '@playwright/test';

export async function capture(
  page: Page,
  info: TestInfo,
  state: string,
  surface: Locator = page.getByRole('dialog'),
) {
  for (const width of [1440, 390, 320]) {
    await page.setViewportSize({ width, height: 900 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
      true,
    );
    await expect
      .poll(() =>
        surface.evaluate((dialog) => {
          const bounds = dialog.getBoundingClientRect();
          return (
            bounds.left >= 0 &&
            bounds.right <= innerWidth + 1 &&
            dialog.scrollWidth <= dialog.clientWidth + 1
          );
        }),
      )
      .toBe(true);
    await page.screenshot({
      path: info.outputPath(`${state}-${width}.png`),
      fullPage: true,
      animations: 'disabled',
    });
  }
}
