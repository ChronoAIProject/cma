import { expect, test } from '@playwright/test';
import { captureDeletion, RedesignDeletionFixture } from './redesign-deletion.fixture';

const sizes = [
  { width: 1440, height: 900, zoom: 1 },
  { width: 390, height: 844, zoom: 1 },
  { width: 1440, height: 900, zoom: 2 },
] as const;

export function registerDeletionConfirmationCases() {
  for (const theme of ['dark', 'light'])
    for (const { width, height, zoom } of sizes) {
      test.describe(`${theme} ${width}px at ${zoom * 100}% browser scale`, () => {
        test.use({ deviceScaleFactor: zoom });
        test(`LOCAL redesign deletion impact is deliberate, readable and dismissible ${theme} ${width}px ${zoom * 100}%`, async ({
          page,
          baseURL,
        }, info) => {
          await page.setViewportSize({ width: width / zoom, height: height / zoom });
          const fixture = new RedesignDeletionFixture();
          await fixture.route(page, baseURL!);
          const dialog = await fixture.openConfirmation(page);
          await page.evaluate(
            ({ theme }) => {
              document.body.toggleAttribute('data-ds-dark-theme', theme === 'dark');
            },
            { theme, zoom },
          );
          const confirm = dialog.getByRole('button', { name: 'Delete Sandbox', exact: true });
          await expect(confirm).toBeDisabled();
          await expect(dialog.getByRole('checkbox')).not.toBeChecked();
          await expect(dialog.getByText(/full impact may not be listed/)).toBeAttached();
          await dialog.getByText('Sample task', { exact: true }).scrollIntoViewIfNeeded();
          await captureDeletion(page, dialog, info, '11-delete-confirm', theme, width, zoom);
          await dialog.getByRole('checkbox').focus();
          await page.keyboard.press('Space');
          await expect(confirm).toBeEnabled();
          await captureDeletion(
            page,
            dialog,
            info,
            '11-delete-confirm-acknowledgement',
            theme,
            width,
            zoom,
          );
          await fixture.settled();
          expect(fixture.commands).toEqual([]);
          await page.keyboard.press('Escape');
          await expect(dialog).toHaveCount(0);
          await fixture.settled();
          const reads = fixture.reads.length;
          await page.clock.runFor(10000);
          await fixture.settled();
          expect(fixture.reads).toHaveLength(reads);
          expect(fixture.commands).toEqual([]);
          fixture.assertNoUnexpected();
        });
      });
    }

  test('LOCAL redesign deletion permission loss withdraws impact and confirmation', async ({
    page,
    baseURL,
  }) => {
    const fixture = new RedesignDeletionFixture();
    await fixture.route(page, baseURL!);
    const dialog = await fixture.openConfirmation(page);
    await dialog.getByRole('checkbox').check();
    await expect(dialog.getByRole('button', { name: 'Delete Sandbox', exact: true })).toBeEnabled();
    fixture.readFailure = 403;
    await page.clock.runFor(1500);
    await expect(dialog.getByText(/resource is no longer accessible/)).toBeVisible();
    await expect(dialog.getByText('Sample task', { exact: true })).toHaveCount(0);
    await expect(
      dialog.getByRole('button', { name: 'Delete Sandbox', exact: true }),
    ).toBeDisabled();
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    expect(fixture.commands).toEqual([]);
    fixture.assertNoUnexpected();
  });

  test('LOCAL redesign deletion lost reply retains the original request across reload and explicit retry', async ({
    page,
    baseURL,
  }) => {
    const fixture = new RedesignDeletionFixture();
    await fixture.route(page, baseURL!);
    fixture.loseResponse = true;
    const dialog = await fixture.openConfirmation(page);
    await dialog.getByRole('checkbox').check();
    await dialog.getByRole('button', { name: 'Delete Sandbox', exact: true }).click();
    const progress = page.getByRole('dialog', { name: 'Deleting Sandbox', exact: true });
    await expect(progress.getByText('Request unconfirmed', { exact: true })).toBeVisible();
    await expect(
      progress.getByRole('button', { name: 'Check request', exact: true }),
    ).toBeEnabled();
    await fixture.settled();
    await page.clock.runFor(15000);
    await fixture.settled();
    expect(fixture.commands).toHaveLength(1);
    const original = fixture.commands[0]!;
    expect(original.key).toBeTruthy();
    await page.reload();
    await expect(progress.getByText('Request unconfirmed', { exact: true })).toBeVisible();
    expect(fixture.commands).toHaveLength(1);
    fixture.loseResponse = false;
    await progress.getByRole('button', { name: 'Check request', exact: true }).click();
    await expect(progress).toHaveCount(0);
    expect(fixture.commands).toEqual([original, original]);
    fixture.assertNoUnexpected();
  });
}

export function registerDeletionStatusCases() {
  for (const theme of ['dark', 'light'])
    for (const { width, height, zoom } of sizes) {
      test.describe(`${theme} ${width}px at ${zoom * 100}% browser scale`, () => {
        test.use({ deviceScaleFactor: zoom });
        test(`LOCAL redesign deletion status and reference ${theme} ${width}px ${zoom * 100}%`, async ({
          page,
          baseURL,
        }, info) => {
          await page.setViewportSize({ width: width / zoom, height: height / zoom });
          const fixture = new RedesignDeletionFixture();
          fixture.boundary.sandbox = { ...fixture.boundary.sandbox, status: 'retiring' };
          await fixture.route(page, baseURL!, true);
          const dialog = await fixture.openStatus(page);
          await page.evaluate(
            ({ theme }) => {
              document.body.toggleAttribute('data-ds-dark-theme', theme === 'dark');
              Object.defineProperty(navigator, 'clipboard', {
                configurable: true,
                value: {
                  writeText: async (value: string) => {
                    sessionStorage.setItem('copied-deletion-reference', value);
                  },
                },
              });
            },
            { theme, zoom },
          );
          await expect(dialog.getByText('Deleting', { exact: true })).toBeVisible();
          await expect(dialog.getByText(/still uses one Sandbox slot/)).toBeVisible();
          await captureDeletion(page, dialog, info, '12-delete-progress', theme, width, zoom);
          await dialog.getByRole('button', { name: 'Copy reference', exact: true }).click();
          await expect(dialog.getByText('Reference copied.', { exact: true })).toBeVisible();
          expect(
            await page.evaluate(() => sessionStorage.getItem('copied-deletion-reference')),
          ).toBe('synthetic-original-delete-reference');
          await page.evaluate(() =>
            Object.defineProperty(navigator, 'clipboard', {
              configurable: true,
              value: {
                writeText: async () => {
                  throw new DOMException('Synthetic denial', 'NotAllowedError');
                },
              },
            }),
          );
          await dialog.getByRole('button', { name: 'Copy reference', exact: true }).click();
          await expect(dialog.getByText(/Copy unavailable/)).toBeVisible();
          await expect(
            dialog.getByText('synthetic-original-delete-reference', { exact: true }),
          ).toBeVisible();
          await page.keyboard.press('Escape');
          await expect(dialog).toHaveCount(0);
          await fixture.settled();
          const reads = fixture.reads.length;
          await page.clock.runFor(10000);
          await fixture.settled();
          expect(fixture.reads).toHaveLength(reads);
          expect(fixture.commands).toEqual([]);
          fixture.assertNoUnexpected();
        });
      });
    }

  for (const status of [403, 404, 503])
    test(`LOCAL redesign deletion ${status} is unconfirmed until retirement is read`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new RedesignDeletionFixture();
      await fixture.route(page, baseURL!, true);
      const dialog = await fixture.openStatus(page);
      await expect(dialog.getByText('Deletion requested', { exact: true })).toBeVisible();
      fixture.readFailure = status;
      await page.clock.runFor(1500);
      await expect(dialog.getByText('Status unavailable', { exact: true })).toBeVisible();
      await expect(dialog.getByRole('button', { name: 'Done', exact: true })).toHaveCount(0);
      await fixture.settled();
      expect(fixture.commands).toEqual([]);
      await page.screenshot({ path: info.outputPath(`12-delete-progress-read-${status}.png`) });
      fixture.readFailure = 0;
      fixture.boundary.sandbox = { ...fixture.boundary.sandbox, status: 'retired' };
      await dialog.getByRole('button', { name: 'Refresh status', exact: true }).click();
      const complete = page.getByRole('dialog', { name: 'Sandbox deleted', exact: true });
      await expect(
        complete.getByText('Cleanup confirmed in this view.', { exact: true }),
      ).toBeVisible();
      await expect(complete.getByText('Deleted', { exact: true })).toBeVisible();
      await page.screenshot({
        path: info.outputPath(`12-delete-progress-confirmed-${status}.png`),
      });
      expect(fixture.commands).toEqual([]);
      fixture.assertNoUnexpected();
    });
}
