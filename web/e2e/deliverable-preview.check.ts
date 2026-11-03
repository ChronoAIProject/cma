import { readFile } from 'node:fs/promises';
import { expect, test } from '@playwright/test';
import { evidence, previewBoundary } from './deliverable-preview-fixtures';

for (const width of [1440, 390]) {
  test.describe(`${width}px`, () => {
    test.use({ viewport: { width, height: 1000 }, hasTouch: width === 390 });
    test('preserves conversation, attachment and pending file save through preview and return focus', async ({
      page,
      baseURL,
    }) => {
      const { boundary, control } = await previewBoundary(page, baseURL!);
      await page.addInitScript(
        (mode) => localStorage.setItem('cma:appearance', mode),
        width === 390 ? 'dark' : 'light',
      );
      await page.goto('/agents/agt_contract');
      const composer = page.getByRole('textbox', { name: 'Message', exact: true });
      await composer.fill('Keep this unsent draft');
      const composerNode = await composer.elementHandle();
      const history = page.locator('.agent-surface__conversation');
      const historyNode = await history.elementHandle();
      await page.getByLabel('Choose attachments').setInputFiles({
        name: 'context.txt',
        mimeType: 'text/plain',
        buffer: Buffer.from('Attachment retained'),
      });
      await expect(
        page.getByRole('list', { name: 'Attachments' }).getByText('Ready', { exact: true }),
      ).toBeVisible();
      const filesButton = page.getByRole('button', { name: 'Show Agent computer', exact: true });
      if (!(await page.getByRole('complementary', { name: 'Agent computer' }).isVisible()))
        await filesButton.click();
      await page.getByRole('button', { name: 'notes.txt', exact: true }).click();
      const editor = page.getByRole('textbox', { name: 'Contents of notes.txt' });
      await editor.fill('Pending saved edit');
      const editorNode = await editor.elementHandle();
      await page.getByRole('button', { name: 'Save', exact: true }).click();
      await expect.poll(() => control.saveStarted).toBe(true);
      await page.getByRole('button', { name: 'Back to conversation', exact: true }).click();
      await page.getByText('Verified deliveries.', { exact: true }).scrollIntoViewIfNeeded();
      const card = page.getByRole('button', { name: 'Preview result.txt', exact: true });
      await card.scrollIntoViewIfNeeded();
      await card.focus();
      const top = await history.evaluate((element) => element.scrollTop);
      if (width === 390) await card.tap();
      else await page.keyboard.press('Enter');
      const preview = page.getByRole('complementary', { name: 'Deliverable preview' });
      await expect(preview.getByRole('textbox', { name: 'Contents of result.txt' })).toHaveValue(
        'Verified answer file',
      );
      await expect(preview.getByRole('button', { name: 'Close preview' })).toBeFocused();
      expect(await editorNode!.evaluate((element) => element.isConnected)).toBe(true);
      const column = page.locator('.frame__details');
      await expect
        .poll(async () => (await column.boundingBox())!.width)
        .toBeGreaterThanOrEqual(300);
      expect((await column.boundingBox())!.width).toBeLessThanOrEqual(520);
      const download = preview.getByRole('button', { name: 'Download result.txt' });
      await download.focus();
      const downloaded = page.waitForEvent('download');
      await page.keyboard.press('Enter');
      expect(await readFile((await (await downloaded).path())!, 'utf8')).toBe(
        'Verified answer file',
      );
      await page.screenshot({ path: `${evidence}/preview-${width}.png`, fullPage: true });
      control.releaseSave();
      await expect.poll(() => control.saved).toBe(true);
      if (width === 390) await preview.getByRole('button', { name: 'Close preview' }).tap();
      else await page.keyboard.press('Escape');
      await expect(preview).not.toBeVisible();
      await expect(card).toBeFocused();
      await expect
        .poll(async () => Math.abs((await history.evaluate((element) => element.scrollTop)) - top))
        .toBeLessThanOrEqual(2);
      expect(
        await composer.evaluate((element, original) => element === original, composerNode),
      ).toBe(true);
      expect(await history.evaluate((element, original) => element === original, historyNode)).toBe(
        true,
      );
      await expect(composer).toHaveValue('Keep this unsent draft');
      await expect(
        page.getByRole('list', { name: 'Attachments' }).getByText('context.txt'),
      ).toBeVisible();
      await filesButton.click();
      await expect(editor).toHaveValue('Pending saved edit');
      expect(await editor.evaluate((element, original) => element === original, editorNode)).toBe(
        true,
      );
      await expect(page.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
      await page.screenshot({ path: `${evidence}/file-save-${width}.png`, fullPage: true });
      await page.getByRole('button', { name: 'Back to conversation', exact: true }).click();
      await page.getByRole('button', { name: 'More Agent actions' }).click();
      await page.getByRole('menuitem', { name: 'Show Agent details', exact: true }).click();
      await expect(page.getByRole('complementary', { name: 'Agent details' })).toBeVisible();
      await page.getByRole('button', { name: 'Close Agent details' }).click();
      await expect(composer).toHaveValue('Keep this unsent draft');
      expect(
        await composer.evaluate((element, original) => element === original, composerNode),
      ).toBe(true);
      expect(await history.evaluate((element, original) => element === original, historyNode)).toBe(
        true,
      );
      await expect(
        page.getByRole('list', { name: 'Attachments' }).getByText('context.txt'),
      ).toBeVisible();
      expect(boundary.errors).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.writes).toEqual([]);
    });

    test('isolates unsafe samples, renders a raster, falls back for PDF and clears revoked content', async ({
      page,
      baseURL,
    }) => {
      const { boundary, control } = await previewBoundary(page, baseURL!);
      await page.goto('/agents/agt_contract');
      if (await page.getByRole('button', { name: 'Back to conversation', exact: true }).isVisible())
        await page.getByRole('button', { name: 'Back to conversation', exact: true }).click();
      const preview = page.getByRole('complementary', { name: 'Deliverable preview' });
      await page.getByText('Verified deliveries.', { exact: true }).scrollIntoViewIfNeeded();
      for (const name of ['report.md', 'unsafe.html', 'unsafe.svg', 'report.pdf', 'pixel.png']) {
        const card = page.getByRole('button', { name: `Preview ${name}`, exact: true });
        if (width === 390) await card.tap();
        else await card.click();
        await expect(preview.getByText('Loading preview…')).not.toBeVisible();
        if (name === 'report.md')
          await expect(preview.getByRole('heading', { name: 'Safe report' })).toBeVisible();
        if (name === 'unsafe.html' || name === 'unsafe.svg')
          await expect(preview.getByRole('textbox')).toHaveValue(/window.previewExecuted/);
        if (name === 'report.pdf')
          await expect(
            preview.getByText('Preview is not supported; download to view this file.'),
          ).toBeVisible();
        if (name === 'pixel.png') {
          const image = preview.getByRole('img', { name: 'pixel.png' });
          await expect(image).toBeVisible();
          expect(await image.evaluate((element: HTMLImageElement) => element.naturalWidth)).toBe(1);
        }
        expect(await preview.locator('script, iframe, object, embed, a[href], svg').count()).toBe(
          0,
        );
        expect(await page.evaluate(() => Object.hasOwn(window, 'previewExecuted'))).toBe(false);
        await page.screenshot({ path: `${evidence}/${name}-${width}.png`, fullPage: true });
        await preview.getByRole('button', { name: 'Close preview' }).click();
        await expect(card).toBeFocused();
      }
      await page.getByRole('button', { name: 'Preview result.txt', exact: true }).click();
      await expect(preview.getByRole('textbox')).toHaveValue('Verified answer file');
      control.revoked = true;
      await page.evaluate(() => window.dispatchEvent(new Event('focus')));
      await expect(preview.getByText('Access unavailable', { exact: true })).toBeVisible();
      await expect(preview.getByRole('textbox')).toHaveCount(0);
      await expect(preview.getByRole('button', { name: 'Download result.txt' })).toBeDisabled();
      await page.screenshot({ path: `${evidence}/revoked-${width}.png`, fullPage: true });
      expect(boundary.errors).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
    });
  });
}
