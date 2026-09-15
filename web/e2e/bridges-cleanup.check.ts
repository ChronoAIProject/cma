import { expect, test } from '@playwright/test';
import { cleanupBoundary } from './bridge-cleanup.fixture';

test('LOCAL Bridge cleanup keeps explicit consent and exact recovery usable on narrow screens', async ({
  page,
  baseURL,
}, info) => {
  const { boundary, row, state } = await cleanupBoundary(page, baseURL!);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto(`/bridges/${row.bridge_id}`);
  await page.getByRole('button', { name: 'Show allocations' }).click();
  await expect(
    page.getByText('No accessible allocations on this page. Continue to the next page.'),
  ).toBeVisible();
  await page.getByRole('button', { name: 'Next allocations' }).click();
  await page.getByRole('button', { name: 'Inspect cleanup for generation 1' }).click();
  await expect(page.getByRole('heading', { name: 'Cleanup not requested' })).toBeVisible();
  expect(state.cursors).toContain('opaque/+/v1?part=1');
  await page.getByRole('button', { name: 'Remove retired child' }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('button', { name: 'Request child removal' })).toBeDisabled();
  await expect(dialog).toContainText(
    'The Workspace, sibling Agents and retained history stay in place.',
  );
  expect(state.writes).toHaveLength(0);
  await page.setViewportSize({ width: 320, height: 900 });
  await expect
    .poll(() =>
      dialog
        .locator('.risk__warning')
        .evaluate((element) => element.scrollWidth <= element.clientWidth),
    )
    .toBe(true);
  await page.screenshot({
    path: info.outputPath('cleanup-consent-320.png'),
    animations: 'disabled',
  });
  await dialog.getByRole('checkbox').check();
  await dialog.getByRole('button', { name: 'Request child removal' }).click();
  await expect(page.getByText(/Cleanup acceptance could not be confirmed/)).toBeVisible();
  expect(state.writes).toHaveLength(1);
  await page.getByRole('button', { name: 'Refresh bridge', exact: true }).click();
  const recover = page.getByRole('button', { name: 'Recover original cleanup request' });
  await expect(recover).toBeEnabled();
  await recover.click();
  await expect(recover).toHaveCount(0);
  await expect(page.getByRole('heading', { name: 'Cleanup requested' })).toBeVisible();
  expect(state.writes).toHaveLength(2);
  expect(state.writes[1]).toEqual(state.writes[0]);
  for (const theme of ['light', 'dark']) {
    await page.evaluate(
      (value) => document.body.toggleAttribute('data-ds-dark-theme', value === 'dark'),
      theme,
    );
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      const section = page.getByRole('region', { name: 'Allocations and child cleanup' });
      await section.scrollIntoViewIfNeeded();
      await expect
        .poll(() =>
          section.evaluate((element) =>
            [...element.querySelectorAll('button,a,dd')].every((item) => {
              const rect = item.getBoundingClientRect();
              return rect.width === 0 || (rect.left >= 0 && rect.right <= innerWidth + 1);
            }),
          ),
        )
        .toBe(true);
      await page.screenshot({
        path: info.outputPath(`cleanup-${theme}-${width}.png`),
        animations: 'disabled',
      });
    }
  }
  state.manager = false;
  await page.getByRole('button', { name: 'Refresh bridge', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Allocations and child cleanup' })).toHaveCount(0);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
