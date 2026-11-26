import { mkdirSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { test, expect } from '@playwright/test';
import { documentId } from './documents.fixture';
import {
  contentOf,
  installClipboard,
  loseSaveResponse,
  openRedesignDocument,
  recoveryOf,
  RedesignDocument,
  replaceContent,
  titleOf,
} from './redesign-document.fixture';

export function registerDocumentRedesignCases() {
  for (const returnToDocument of [false, true]) {
    test(`LOCAL redesign document retries the committed save without losing later edits (return: ${returnToDocument})`, async ({
      page,
      baseURL,
    }) => {
      await installClipboard(page);
      const fixture = await openRedesignDocument(page, baseURL!);
      await loseSaveResponse(page, fixture);
      const original = structuredClone(fixture.writes[0]);
      await page.getByRole('button', { name: 'Preview', exact: true }).click();
      const retry = page.getByRole('button', { name: 'Retry same save' });
      await expect(titleOf(page)).toBeDisabled();
      await expect(contentOf(page)).toHaveAttribute('contenteditable', 'false');
      await expect(retry).toBeDisabled();
      await retry.dispatchEvent('click');
      await contentOf(page).dispatchEvent('keydown', { key: 's', ctrlKey: true });
      await contentOf(page).dispatchEvent('keydown', { key: 's', metaKey: true });
      await page.clock.runFor(30_000);
      expect(fixture.writes).toHaveLength(1);
      await page.getByRole('button', { name: 'Edit', exact: true }).click();
      await expect(titleOf(page)).toBeEnabled();
      await expect(retry).toBeEnabled();
      await titleOf(page).fill('Newer unsubmitted title');
      await replaceContent(page, 'Newer unsubmitted body');
      if (returnToDocument) {
        await page.getByRole('button', { name: 'Back to artifacts' }).click();
        await page.getByRole('button', { name: 'Leave without saving', exact: true }).click();
        await page.getByRole('link', { name: /Weekly project summary/ }).click();
        await expect(titleOf(page)).toHaveValue('Newer unsubmitted title');
        await expect(contentOf(page)).toHaveText('Newer unsubmitted body');
      }
      await page.clock.runFor(30_000);
      expect(fixture.writes).toHaveLength(1);
      await page.getByRole('button', { name: 'Copy all text' }).click();
      await expect(page.getByText('All text copied.')).toBeVisible();
      expect(
        await page.evaluate(
          () => (window as unknown as { copiedDocumentText: string }).copiedDocumentText,
        ),
      ).toBe('# Newer unsubmitted body\n\n');
      let finish!: () => void;
      fixture.saveGate = new Promise((resolve) => {
        finish = resolve;
      });
      await page.getByRole('button', { name: 'Retry same save' }).click();
      await expect.poll(() => fixture.writes.length).toBe(2);
      await expect(page.getByRole('button', { name: 'Retry same save' })).toBeDisabled();
      await page.getByRole('button', { name: 'Retry same save' }).dispatchEvent('click');
      await contentOf(page).dispatchEvent('keydown', { key: 's', ctrlKey: true });
      await contentOf(page).dispatchEvent('keydown', { key: 's', metaKey: true });
      finish();
      await expect(page.locator('.document-workspace__save-status')).toHaveText('Unsaved changes');
      expect(fixture.writes).toHaveLength(2);
      expect(fixture.writes[1]).toEqual(original);
      expect(fixture.applications).toBe(1);
      await expect(titleOf(page)).toHaveValue('Newer unsubmitted title');
      await expect(contentOf(page)).toHaveText('Newer unsubmitted body');
      await page.getByRole('button', { name: 'Back to artifacts' }).click();
      await page.getByRole('button', { name: 'Stay', exact: true }).click();
      await page.getByRole('button', { name: 'Save', exact: true }).click();
      await expect(page.locator('.document-workspace__save-status')).toHaveText('Saved');
      expect(fixture.writes).toHaveLength(3);
      expect(fixture.writes[2]?.key).not.toBe(original?.key);
      expect(fixture.writes[2]?.body).toEqual({
        title: 'Newer unsubmitted title',
        markdown: '# Newer unsubmitted body\n\n',
        expected_revision: 4,
      });
      expect(fixture.applications).toBe(2);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
    });
  }

  test('LOCAL redesign document copy refusal keeps selectable current text and the original save', async ({
    page,
    baseURL,
  }) => {
    await installClipboard(page, true);
    const fixture = await openRedesignDocument(page, baseURL!);
    await loseSaveResponse(page, fixture);
    await replaceContent(page, 'Current text to recover');
    await page.getByRole('button', { name: 'Copy all text' }).click();
    const fallback = page.getByRole('textbox', { name: 'Document text to copy' });
    await expect(fallback).toHaveValue('# Current text to recover\n\n');
    await expect(page.getByText('Copy failed. Select and copy the text below.')).toBeVisible();
    await fallback.focus();
    expect(
      await fallback.evaluate(
        (node: HTMLTextAreaElement) => node.selectionEnd - node.selectionStart,
      ),
    ).toBe('# Current text to recover\n\n'.length);
    await replaceContent(page, 'A newer draft after copy failure');
    await expect(fallback).toHaveValue('# A newer draft after copy failure\n\n');
    expect(fixture.writes).toHaveLength(1);
    await expect(page.getByRole('button', { name: 'Retry same save' })).toBeEnabled();
  });

  for (const status of [401, 403, 409, 503]) {
    test(`LOCAL redesign document preserves the existing ${status} save classification`, async ({
      page,
      baseURL,
    }) => {
      const fixture = new RedesignDocument();
      fixture.refusal = status;
      await openRedesignDocument(page, baseURL!, fixture);
      await titleOf(page).fill('Unsubmitted recovery title');
      await page.getByRole('button', { name: 'Save', exact: true }).click();
      if (status === 401 || status === 403) {
        await expect(contentOf(page)).toHaveCount(0);
        await expect(recoveryOf(page)).toHaveCount(0);
        await expect(page.getByRole('alert')).toContainText(`Save refused (${status})`);
      } else if (status === 409) {
        await expect(page.locator('.document-workspace__save-status')).toHaveText(
          'Version changed',
        );
        await expect(page.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
        await expect(titleOf(page)).toHaveValue('Unsubmitted recovery title');
        await expect(recoveryOf(page)).toHaveCount(0);
      } else {
        await expect(recoveryOf(page)).toBeVisible();
        await expect(titleOf(page)).toBeEnabled();
        await expect(page.getByRole('button', { name: 'Retry same save' })).toBeEnabled();
      }
      await page.clock.runFor(30_000);
      expect(fixture.writes).toHaveLength(1);
      expect(fixture.applications).toBe(0);
      expect(fixture.boundary.errors).toEqual([]);
    });
  }

  for (const theme of ['dark', 'light']) {
    test(`LOCAL redesign document screen 16 ${theme}, narrow and 200 percent layout`, async ({
      page,
      baseURL,
    }, info) => {
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
      const fixture = await openRedesignDocument(page, baseURL!);
      await loseSaveResponse(page, fixture);
      if (theme === 'dark')
        await expect(page.locator('body')).toHaveAttribute('data-ds-dark-theme', '');
      else await expect(page.locator('body')).not.toHaveAttribute('data-ds-dark-theme');
      const folder =
        process.env.CMA_REDESIGN_SCREENSHOT_DIR ?? info.outputPath('redesign-document');
      const images = [];
      const metrics = await page.context().newCDPSession(page);
      for (const layout of ['desktop', 'narrow', 'zoom-200']) {
        const viewport =
          layout === 'narrow'
            ? { width: 390, height: 844 }
            : layout === 'zoom-200'
              ? { width: 720, height: 450 }
              : { width: 1440, height: 900 };
        await page.setViewportSize(viewport);
        if (layout !== 'desktop')
          await titleOf(page).fill(
            'Weekly project summary with a long title to check that every recovery action remains reachable',
          );
        const retry = page.getByRole('button', { name: 'Retry same save' });
        await retry.scrollIntoViewIfNeeded();
        await expect(retry).toBeVisible();
        await expect(page.getByRole('button', { name: 'Copy all text' })).toBeVisible();
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
        ).toBe(true);
        await titleOf(page).focus();
        for (
          let tab = 0;
          tab < 18 && !(await retry.evaluate((node) => node === document.activeElement));
          tab++
        )
          await page.keyboard.press('Tab');
        await expect(retry).toBeFocused();
        const copy = page.getByRole('button', { name: 'Copy all text' });
        await expect(retry).toBeInViewport({ ratio: 1 });
        await expect(copy).toBeInViewport({ ratio: 1 });
        for (const element of [recoveryOf(page), retry, copy, titleOf(page), contentOf(page)]) {
          const bounds = await element.boundingBox();
          expect(bounds).not.toBeNull();
          expect(bounds!.x).toBeGreaterThanOrEqual(0);
          expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(viewport.width);
        }
        const name =
          layout === 'desktop' ? '16-document-save.png' : `16-document-save-${layout}.png`;
        mkdirSync(join(folder, theme), { recursive: true });
        const path = join(folder, theme, name);
        if (layout === 'zoom-200') {
          const capture = await metrics.send('Page.captureScreenshot', {
            format: 'png',
            clip: { x: 0, y: 0, ...viewport, scale: 2 },
            captureBeyondViewport: false,
          });
          const png = Buffer.from(capture.data, 'base64');
          expect([png.readUInt32BE(16), png.readUInt32BE(20)]).toEqual([1440, 900]);
          writeFileSync(path, png);
        } else await page.screenshot({ path, animations: 'disabled' });
        images.push({
          path,
          theme,
          layout,
          viewport,
          rasterScale: layout === 'zoom-200' ? 2 : 1,
          zoomMethod:
            layout === 'zoom-200' ? 'Equivalent 200% layout viewport at 2x raster scale' : 'none',
          reducedMotion: true,
        });
      }
      writeFileSync(
        join(folder, `${theme}-screens.json`),
        JSON.stringify(
          {
            source: process.env.CMA_REDESIGN_HEAD ?? 'working tree',
            route: `/artifacts/${documentId}`,
            fixture: 'committed save response lost; original request retained',
            images,
          },
          null,
          2,
        ),
      );
      expect(fixture.writes).toHaveLength(1);
      expect(fixture.applications).toBe(1);
    });
  }
}
