import { expect, type Page } from '@playwright/test';

/** The fixture is a 1280×720 JPEG with a white bottom margin, in either app theme. */
export async function captureSingleBrowserFrame(page: Page, path?: string) {
  const panel = page.getByRole('tabpanel', { name: 'Screen', exact: true });
  const frame = panel.getByRole('img', { name: 'Agent browser page', exact: true });
  await expect(panel).toBeVisible();
  await expect(page.getByRole('img', { name: 'Agent browser page', exact: true })).toHaveCount(1);
  await expect(panel.getByRole('figure', { name: 'Live browser page', exact: true })).toBeVisible();
  await expect(frame).toBeVisible();
  await expect(panel.locator('canvas:visible')).toHaveCount(0);
  await expect
    .poll(() =>
      frame.evaluate(
        (image: HTMLImageElement) =>
          image.complete && image.naturalWidth === 1280 && image.naturalHeight === 720,
      ),
    )
    .toBe(true);
  const measured: { bounds?: NonNullable<Awaited<ReturnType<typeof frame.boundingBox>>> } = {};
  await expect
    .poll(async () => {
      const image = await frame.boundingBox();
      const container = await panel.boundingBox();
      if (!image || !container) return false;
      const fits =
        image.x >= container.x - 1 &&
        image.y >= container.y - 1 &&
        image.x + image.width <= container.x + container.width + 1 &&
        image.y + image.height <= container.y + container.height + 1 &&
        Math.abs(image.width - container.width) < 1 &&
        Math.abs(image.height - (image.width * 9) / 16) < 1;
      if (fits) measured.bounds = image;
      return fits;
    })
    .toBe(true);
  const bounds = measured.bounds!;
  const screenshot = await page.screenshot({ path });
  // Check the viewport capture, not a separate element capture which could repaint it.
  // A duplicated/clipped top strip in the footer is grey/text instead of this white margin.
  const footer = await page.evaluate(
    async ({ png, bounds, viewportWidth }) => {
      const image = new Image();
      image.src = `data:image/png;base64,${png}`;
      await image.decode();
      const canvas = document.createElement('canvas');
      canvas.width = image.naturalWidth;
      canvas.height = image.naturalHeight;
      const context = canvas.getContext('2d')!;
      context.drawImage(image, 0, 0);
      const scale = image.naturalWidth / viewportWidth;
      return [0.1, 0.5, 0.9].map((fraction) =>
        [
          ...context.getImageData(
            Math.floor((bounds.x + bounds.width * fraction) * scale),
            Math.floor((bounds.y + bounds.height - 2) * scale),
            1,
            1,
          ).data,
        ].slice(0, 3),
      );
    },
    { png: screenshot.toString('base64'), bounds, viewportWidth: page.viewportSize()!.width },
  );
  expect(
    footer.every((rgb) => rgb.every((channel) => channel >= 250)),
    JSON.stringify(footer),
  ).toBe(true);
}
