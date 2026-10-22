import { test, expect } from '@playwright/test';
import { LocalDocuments, documentId } from './documents.fixture';
for (const width of [1600, 390]) {
  for (const theme of ['light', 'dark']) {
    test(`LOCAL document gallery, rich editing, comments and sharing at ${width}px ${theme}`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalDocuments();
      await page.setViewportSize({ width, height: 1000 });
      await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
      await fixture.route(page, baseURL!);
      await page.goto('/artifacts');
      await page.evaluate(
        (value) => document.body.toggleAttribute('data-ds-dark-theme', value === 'dark'),
        theme,
      );
      await expect(page.getByRole('heading', { name: 'Artifacts', exact: true })).toBeVisible();
      await expect(page.getByRole('link', { name: /Product brief/ })).toBeVisible();
      await page.screenshot({ path: info.outputPath(`gallery-${width}-${theme}.png`) });
      await page.getByRole('link', { name: /Product brief/ }).click();
      const content = page.getByRole('textbox', { name: 'Document content', exact: true });
      await expect(content).toBeVisible();
      await expect(content.getByRole('table')).toBeVisible();
      await expect(content.getByRole('checkbox').first()).toBeChecked();
      await expect(page.locator('.document-diagram img')).toBeVisible();
      await expect(page.locator('.document-diagram img')).toHaveAttribute('src', /Research/);
      await expect
        .poll(() =>
          page
            .locator('.document-diagram img')
            .evaluate((node: HTMLImageElement) => node.complete && node.naturalWidth > 0),
        )
        .toBe(true);
      await expect(page.locator('[data-type=detailsContent]')).toContainText(
        'Useful hidden context.',
      );
      await page.screenshot({ path: info.outputPath(`document-${width}-${theme}.png`) });
      const originalEditor = await content.elementHandle();
      await content.click();
      await content.press('ControlOrMeta+End');
      await content.press('Enter');
      await page.keyboard.type('A locally verified rich edit.');
      await page.getByRole('button', { name: 'Preview', exact: true }).click();
      await expect(content).toHaveAttribute('contenteditable', 'false');
      await expect(page.getByRole('toolbar', { name: 'Document formatting' })).toHaveCount(0);
      await page.getByRole('button', { name: 'On this page', exact: true }).click();
      const outline = page.getByRole('navigation', { name: 'On this page' });
      await outline.getByRole('button', { name: /Product brief$/ }).click();
      await expect(content.getByRole('heading', { name: /Product brief$/ })).toBeInViewport();
      await expect(page.locator('.document-workspace__save-status')).toHaveText('Unsaved changes');
      expect(fixture.writes).toHaveLength(0);
      await page.screenshot({ path: info.outputPath(`preview-${width}-${theme}.png`) });
      await page.getByRole('button', { name: 'Edit', exact: true }).click();
      expect(await content.evaluate((node, original) => node === original, originalEditor)).toBe(
        true,
      );
      await expect(content).toHaveAttribute('contenteditable', 'true');
      await expect(content).toContainText('A locally verified rich edit.');
      await page.getByRole('button', { name: 'On this page', exact: true }).click();
      await page.getByRole('button', { name: 'Save', exact: true }).click();
      await expect(page.locator('.document-workspace__save-status')).toHaveText('Saved');
      expect(fixture.markdown).toContain('A locally verified rich edit.');
      expect(fixture.markdown).toContain('mermaid');
      expect(fixture.markdown).toContain('[font family="serif"]');
      await page.getByRole('button', { name: 'Comments', exact: true }).click();
      const comments = page.getByRole('complementary', { name: 'Document comments' });
      await comments.getByRole('textbox', { name: 'Add a comment' }).fill('Looks ready to review.');
      await comments.getByRole('button', { name: 'Comment', exact: true }).click();
      await expect(comments.getByText('Looks ready to review.')).toBeVisible();
      await page.screenshot({ path: info.outputPath(`editor-${width}-${theme}.png`) });
      await comments.getByRole('button', { name: 'Close comments' }).click();
      await page.getByRole('button', { name: 'Share', exact: true }).click();
      await page.getByRole('textbox', { name: 'Recipient user ID' }).fill('reviewer');
      await page.getByRole('button', { name: 'Add person' }).click();
      await expect(page.getByRole('list', { name: 'People with access' })).toContainText(
        'reviewer',
      );
      await page.screenshot({ path: info.outputPath(`sharing-${width}-${theme}.png`) });
      await page.getByRole('button', { name: 'Create view link' }).click();
      const link = await page.getByRole('textbox', { name: 'Document share link' }).inputValue();
      expect(link).toMatch(/#token=[a-f0-9]{64}$/);
      expect(fixture.writes.every((write) => Boolean(write.key))).toBe(true);
      await page.getByRole('button', { name: 'Done', exact: true }).click();
      await page.getByRole('button', { name: 'Back to artifacts' }).click();
      await page.getByRole('link', { name: /Product brief/ }).click();
      await expect(content).toContainText('A locally verified rich edit.');
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
        .toBe(true);
      expect(fixture.boundary.errors).toEqual([]);
    });
  }
}
test('LOCAL shared document keeps fragment in headers and disables content editing', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  fixture.document.access = 'read';
  await fixture.route(page, baseURL!);
  await page.goto(`/artifacts/${documentId}#token=${'b'.repeat(64)}`);
  await expect(page.getByRole('textbox', { name: 'Document content' })).toHaveAttribute(
    'contenteditable',
    'false',
  );
  await expect(page.getByRole('button', { name: 'Save', exact: true })).toHaveCount(0);
  expect(page.url()).not.toContain('#token');
  expect(
    fixture.reads
      .filter((read) => read.path.endsWith(documentId) || read.path.endsWith('/source'))
      .every((read) => read.token === 'b'.repeat(64) && !read.query.includes('token')),
  ).toBe(true);
  await page.getByRole('button', { name: 'Comments', exact: true }).click();
  await expect(page.getByText('You have view access to this document.')).toBeVisible();
  expect(fixture.writes).toHaveLength(0);
});

test('LOCAL create, formatting commands and image paste survive save and reopen', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  fixture.exists = false;
  await fixture.route(page, baseURL!);
  await page.goto('/artifacts');
  await page.getByRole('button', { name: 'New document', exact: true }).click();
  await page
    .getByRole('dialog', { name: 'New document' })
    .getByRole('textbox')
    .fill('Created here');
  await page.getByRole('button', { name: 'Create document', exact: true }).click();
  const content = page.getByRole('textbox', { name: 'Document content', exact: true });
  await expect(content).toBeVisible();
  await content.press('ControlOrMeta+A');
  await content.pressSequentially('Hello document');
  await page.getByRole('button', { name: 'Font', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Serif', exact: true }).click();
  await content.pressSequentially(' Serif text');
  await content.press('Enter');
  await page.getByRole('button', { name: 'Insert content', exact: true }).click();
  await page.getByRole('menuitem', { name: 'Dropdown section', exact: true }).click();
  await expect(content.locator('[data-type=detailsContent]')).toContainText('Add details here.');
  await content.press('ArrowDown');
  await content.press('Enter');
  await content.evaluate((node) => {
    const bytes = Uint8Array.from(
      atob(
        'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVQIHWP4z8DwHwAFgAI/ScLbtAAAAABJRU5ErkJggg==',
      ),
      (character) => character.charCodeAt(0),
    );
    const data = new DataTransfer();
    data.items.add(new File([bytes], 'pasted.png', { type: 'image/png' }));
    node.dispatchEvent(
      new ClipboardEvent('paste', { clipboardData: data, bubbles: true, cancelable: true }),
    );
  });
  await expect(content.getByRole('img', { name: 'pasted.png' })).toBeVisible();
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(page.locator('.document-workspace__save-status')).toHaveText('Saved');
  expect(fixture.markdown).toContain(':::details');
  expect(fixture.markdown).toContain('data:image/png;base64,');
  expect(fixture.markdown).toContain('[font family="serif"]');
  await page.getByRole('button', { name: 'Back to artifacts' }).click();
  await page.getByRole('link', { name: /Created here/ }).click();
  await expect(content).toContainText('Hello document');
  await expect(content.getByRole('img', { name: 'pasted.png' })).toBeVisible();
});
