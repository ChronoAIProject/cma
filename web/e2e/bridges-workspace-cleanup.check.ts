import { expect, test } from '@playwright/test';
import { workspaceCleanupBoundary } from './bridge-workspace-cleanup.fixture';

test('LOCAL owned Workspace cleanup keeps separate consent and exact recovery through refresh at narrow widths', async ({
  page,
  baseURL,
}, info) => {
  const { boundary, row, state, workspace } = await workspaceCleanupBoundary(page, baseURL!);
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto(`/bridges/${row.bridge_id}`);
  await page.getByRole('button', { name: 'Show allocations' }).click();
  await page.getByRole('button', { name: 'Inspect cleanup for generation 1' }).click();
  await expect(page.getByRole('heading', { name: 'Child cleanup complete' })).toBeVisible();
  await page.getByRole('button', { name: 'Inspect Workspace cleanup' }).click();
  await expect(
    page.getByRole('heading', { name: 'Workspace cleanup not requested' }),
  ).toBeVisible();
  await page.getByRole('button', { name: 'Remove owned Workspace', exact: true }).click();
  const dialog = page.getByRole('dialog');
  await expect(dialog.getByRole('button', { name: 'Request removal' })).toBeDisabled();
  await expect(dialog).toContainText(row.target.workspace_id);
  await expect(dialog).toContainText(
    'The Sandbox, sibling Workspaces and retained history stay in place.',
  );
  expect(workspace.writes).toHaveLength(0);
  expect(workspace.childWrites).toBe(0);
  await page.setViewportSize({ width: 320, height: 900 });
  await expect
    .poll(() =>
      dialog
        .locator('.risk__warning')
        .evaluate((element) => element.scrollWidth <= element.clientWidth),
    )
    .toBe(true);
  await expect
    .poll(() =>
      dialog.locator('button').evaluateAll((buttons) =>
        buttons.every((button) => {
          const range = document.createRange();
          range.selectNodeContents(button);
          const text = range.getBoundingClientRect();
          const bounds = button.getBoundingClientRect();
          return text.top >= bounds.top && text.bottom <= bounds.bottom;
        }),
      ),
    )
    .toBe(true);
  await page.screenshot({
    path: info.outputPath('workspace-consent-320.png'),
    animations: 'disabled',
  });
  await dialog.getByRole('checkbox').check();
  await dialog.getByRole('button', { name: 'Request removal' }).click();
  await expect(page.getByText(/Workspace cleanup acceptance could not be confirmed/)).toBeVisible();
  expect(workspace.writes).toHaveLength(1);
  await page.getByRole('button', { name: 'Next allocations' }).click();
  expect(workspace.cursors).toContain('workspace/next?opaque=1');
  workspace.bridgeRevision = 99;
  workspace.workspaceRevision = 9;
  workspace.runtimeSupported = false;
  await page.getByRole('button', { name: 'Refresh bridge', exact: true }).click();
  const recover = page.getByRole('button', { name: 'Recover original Workspace cleanup request' });
  await expect(recover).toBeEnabled();
  await expect(
    page.getByText('This Sandbox does not currently support safe owned Workspace cleanup.'),
  ).toBeVisible();
  await expect(
    page.getByRole('button', { name: 'Remove owned Workspace', exact: true }),
  ).toBeDisabled();
  const recoverySection = page.getByRole('region', { name: 'Owned Workspace cleanup' });
  await expect
    .poll(() =>
      recoverySection.locator('button').evaluateAll((buttons) =>
        buttons.every((button) => {
          const range = document.createRange();
          range.selectNodeContents(button);
          const text = range.getBoundingClientRect();
          const bounds = button.getBoundingClientRect();
          return text.top >= bounds.top && text.bottom <= bounds.bottom;
        }),
      ),
    )
    .toBe(true);
  await recover.scrollIntoViewIfNeeded();
  await expect(recover).toBeInViewport();
  await page.screenshot({
    path: info.outputPath('workspace-recovery-320.png'),
    animations: 'disabled',
  });
  await recover.click();
  await expect(recover).toHaveCount(0);
  expect(workspace.writes).toHaveLength(2);
  expect(workspace.writes[1]).toEqual(workspace.writes[0]);
  expect(workspace.childWrites).toBe(0);
  for (const theme of ['light', 'dark']) {
    await page.evaluate(
      (value) => document.body.toggleAttribute('data-ds-dark-theme', value === 'dark'),
      theme,
    );
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      const section = page.getByRole('region', { name: 'Owned Workspace cleanup' });
      await section.scrollIntoViewIfNeeded();
      await expect
        .poll(() =>
          section.evaluate((element) =>
            [...element.querySelectorAll('button,a,p')].every((item) => {
              const bounds = item.getBoundingClientRect();
              const range = document.createRange();
              range.selectNodeContents(item);
              const text = range.getBoundingClientRect();
              return (
                bounds.width === 0 ||
                (bounds.left >= 0 &&
                  bounds.right <= innerWidth + 1 &&
                  item.scrollWidth <= item.clientWidth + 1 &&
                  (item.tagName !== 'BUTTON' ||
                    (text.top >= bounds.top && text.bottom <= bounds.bottom)))
              );
            }),
          ),
        )
        .toBe(true);
      await section.screenshot({
        path: info.outputPath(`workspace-${theme}-${width}.png`),
        animations: 'disabled',
      });
    }
  }
  workspace.complete = true;
  await page.getByRole('button', { name: 'Refresh Workspace cleanup' }).click();
  await expect(page.getByRole('heading', { name: 'Workspace cleanup complete' })).toBeVisible();
  await expect(page.getByText('The owned Workspace reservation has been released.')).toBeVisible();
  state.manager = false;
  await page.getByRole('button', { name: 'Refresh bridge', exact: true }).click();
  await expect(page.getByRole('region', { name: 'Owned Workspace cleanup' })).toHaveCount(0);
  expect(workspace.writes).toHaveLength(2);
  expect(workspace.childWrites).toBe(0);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
