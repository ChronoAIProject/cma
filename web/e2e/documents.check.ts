import { test, expect } from '@playwright/test';
import './documents.autolink';
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
      await page.screenshot({
        animations: 'disabled',
        path: info.outputPath(`gallery-${width}-${theme}.png`),
      });
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
      await page.screenshot({
        animations: 'disabled',
        path: info.outputPath(`document-${width}-${theme}.png`),
      });
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
      await expect(page.locator('.document-workspace__save-status')).toHaveText(
        'Unsaved changes · Switch to Edit to save',
      );
      expect(fixture.writes).toHaveLength(0);
      await page.screenshot({
        animations: 'disabled',
        path: info.outputPath(`preview-${width}-${theme}.png`),
      });
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
      await page.screenshot({
        animations: 'disabled',
        path: info.outputPath(`editor-${width}-${theme}.png`),
      });
      await comments.getByRole('button', { name: 'Close comments' }).click();
      await page.getByRole('button', { name: 'Share', exact: true }).click();
      const search = page.getByRole('textbox', { name: 'Find share recipients' });
      const add = page.getByRole('button', { name: 'Add person' });
      const select = page.getByRole('button', { name: 'Select Local Reviewer (Reviewer-A)' });
      await search.fill('unresolved');
      await expect(page.getByText('No matching people.')).toBeVisible();
      await expect(add).toBeDisabled();
      await search.fill('reviewer');
      await select.click();
      await expect(page.getByRole('list', { name: 'Selected recipients' })).toContainText(
        'Reviewer-A',
      );
      await search.fill('review');
      await expect(add).toBeDisabled();
      await select.click();
      await add.click();
      await expect(page.getByRole('list', { name: 'People with access' })).toContainText(
        'Reviewer-A',
      );
      await page.screenshot({
        animations: 'disabled',
        path: info.outputPath(`sharing-${width}-${theme}.png`),
      });
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

test('LOCAL unresolved document recipients leave sharing unchanged', async ({ page, baseURL }) => {
  const fixture = new LocalDocuments();
  fixture.markdown = '# Product brief';
  await fixture.route(page, baseURL!);
  await page.goto(`/artifacts/${documentId}`);
  await page.getByRole('button', { name: 'Share', exact: true }).click();
  const search = page.getByRole('textbox', { name: 'Find share recipients' });
  const add = page.getByRole('button', { name: 'Add person' });
  const grants = page.getByRole('list', { name: 'People with access' }).getByRole('listitem');
  for (const subject of ['missing.recipient@example.test', 'definitely-not-a-nyxid-user']) {
    const lookup = page.waitForResponse((response) => {
      const url = new URL(response.url());
      return url.pathname.endsWith('/share-recipients') && url.searchParams.get('q') === subject;
    });
    await search.fill(subject);
    const response = await lookup;
    expect(response.ok()).toBe(true);
    expect(await response.json()).toEqual([]);
    await expect(page.getByText('No matching people.')).toBeVisible();
    await expect(search).toHaveValue(subject);
    await expect(add).toBeDisabled();
    await search.press('Enter');
    await expect(grants).toHaveCount(0);
  }
  expect(fixture.writes).toEqual([]);

  await search.fill('reviewer');
  await page.getByRole('button', { name: 'Select Local Reviewer (Reviewer-A)' }).click();
  await add.click();
  await expect(grants).toHaveCount(1);
  await expect(grants.first().getByText('Reviewer-A', { exact: true })).toBeVisible();
  await expect(grants.first().getByText('Can view', { exact: true })).toBeVisible();
  expect(fixture.writes).toEqual([
    expect.objectContaining({
      method: 'PUT',
      path: `/api/v1/documents/${documentId}/shares/Reviewer-A`,
    }),
  ]);
  await page.reload();
  await page.getByRole('button', { name: 'Share', exact: true }).click();
  await expect(grants).toHaveCount(1);
  await expect(grants.first()).toContainText('Reviewer-A');
  expect(fixture.boundary.errors).toEqual([]);
});

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
  await expect(page.getByRole('textbox', { name: 'Document title' })).toBeDisabled();
  await expect(page.getByRole('group', { name: 'Document mode' })).toHaveCount(0);
  const content = page.getByRole('textbox', { name: 'Document content', exact: true });
  await content.click();
  await page.keyboard.press('Control+s');
  await page.keyboard.press('Meta+s');
  expect(page.url()).not.toContain('#token');
  expect(
    fixture.reads
      .filter((read) => read.path.endsWith(documentId) || read.path.endsWith('/source'))
      .every((read) => read.token === 'b'.repeat(64) && !read.query.includes('token')),
  ).toBe(true);
  await page.getByRole('button', { name: 'Comments', exact: true }).click();
  await expect(page.getByText('You have view access to this document.')).toBeVisible();
  expect(fixture.writes).toHaveLength(0);
  expect(fixture.boundary.errors).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
});

for (const shortcut of ['Control+s', 'Meta+s']) {
  test(`LOCAL Preview blocks title focus and ${shortcut} while retaining drafts through navigation`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalDocuments();
    fixture.markdown = '# Product brief\n\nSaved body.\n';
    await fixture.route(page, baseURL!);
    await page.goto(`/artifacts/${documentId}`);
    const title = page.getByRole('textbox', { name: 'Document title' });
    const content = page.getByRole('textbox', { name: 'Document content', exact: true });
    const status = page.locator('.document-workspace__save-status');
    const save = page.getByRole('button', { name: 'Save', exact: true });
    const edit = page.getByRole('button', { name: 'Edit', exact: true });
    const preview = page.getByRole('button', { name: 'Preview', exact: true });
    const back = page.getByRole('button', { name: 'Back to artifacts' });
    await expect(content).toBeVisible();
    await preview.click();
    await expect(title).toBeDisabled();
    await expect(content).toHaveAttribute('contenteditable', 'false');
    await expect(save).toBeDisabled();
    await back.focus();
    await title.evaluate((node: HTMLInputElement) => node.focus());
    await expect(back).toBeFocused();
    await page.keyboard.type('AccidentalTitle');
    await page.keyboard.press('Tab');
    await expect(edit).toBeFocused();
    await expect(title).toHaveValue('Product brief');
    const cleanBody = await content.textContent();
    await content.click();
    await page.keyboard.type('Accidental body');
    await page.keyboard.press(shortcut);
    await expect(content).toHaveText(cleanBody!);
    await expect(status).toHaveText('Saved');
    expect(fixture.writes).toHaveLength(0);
    await page.reload();
    await expect(title).toHaveValue('Product brief');
    await expect(content).toHaveText(cleanBody!);

    await edit.click();
    await title.fill('Draft title');
    await content.click();
    await content.press('ControlOrMeta+End');
    await content.press('Enter');
    await page.keyboard.type('Retained draft body.');
    await preview.click();
    await expect(title).toHaveValue('Draft title');
    await expect(title).toBeDisabled();
    await expect(content).toContainText('Retained draft body.');
    await expect(status).toHaveText('Unsaved changes · Switch to Edit to save');
    await expect(save).toBeDisabled();
    await save.dispatchEvent('click');
    await content.click();
    await page.keyboard.press(shortcut);
    await content.dispatchEvent('keydown', {
      key: 's',
      ctrlKey: shortcut === 'Control+s',
      metaKey: shortcut === 'Meta+s',
    });
    expect(fixture.writes).toHaveLength(0);

    await back.click();
    await page.getByRole('button', { name: 'Leave without saving', exact: true }).click();
    await page.getByRole('link', { name: /Product brief/ }).click();
    await expect(title).toHaveValue('Draft title');
    await expect(title).toBeEditable();
    await expect(content).toContainText('Retained draft body.');
    await expect(content).toHaveAttribute('contenteditable', 'true');
    expect(fixture.writes).toHaveLength(0);
    await content.click();
    await page.keyboard.press(shortcut);
    await expect(status).toHaveText('Saved');
    expect(fixture.writes).toHaveLength(1);
    expect(fixture.writes[0]).toMatchObject({
      method: 'PATCH',
      path: `/api/v1/documents/${documentId}`,
      body: {
        title: 'Draft title',
        markdown: expect.stringContaining('Retained draft body.'),
        expected_revision: 3,
      },
      key: expect.any(String),
    });
    await preview.click();
    await content.click();
    await page.keyboard.press(shortcut);
    await page.reload();
    await expect(title).toHaveValue('Draft title');
    await expect(content).toContainText('Retained draft body.');
    await expect(status).toHaveText('Saved');
    await back.click();
    await page.getByRole('link', { name: /Draft title/ }).click();
    await expect(title).toHaveValue('Draft title');
    await expect(content).toContainText('Retained draft body.');
    expect(fixture.writes).toHaveLength(1);
    expect(fixture.boundary.errors).toEqual([]);
    expect(fixture.boundary.unexpected).toEqual([]);
  });
}

test('LOCAL Preview refuses an uncertain save retry and Edit retains its exact body and key', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  fixture.markdown = '# Saved body';
  fixture.saveFailures = 1;
  await fixture.route(page, baseURL!);
  await page.goto(`/artifacts/${documentId}`);
  const content = page.getByRole('textbox', { name: 'Document content', exact: true });
  const title = page.getByRole('textbox', { name: 'Document title' });
  await expect(content).toBeVisible();
  await title.fill('Unconfirmed title');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  const retry = page.getByRole('button', { name: 'Retry same save', exact: true });
  await expect(retry).toBeEnabled();
  expect(fixture.writes).toHaveLength(1);
  const original = fixture.writes[0];
  expect(original?.key).toBeTruthy();
  await page.getByRole('button', { name: 'Preview', exact: true }).click();
  await expect(title).toBeDisabled();
  await expect(content).toHaveAttribute('contenteditable', 'false');
  await expect(retry).toBeDisabled();
  await retry.dispatchEvent('click');
  await content.dispatchEvent('keydown', { key: 's', ctrlKey: true });
  await content.dispatchEvent('keydown', { key: 's', metaKey: true });
  await expect(page.locator('.document-workspace__save-status')).toHaveText(
    'Save not confirmed · Switch to Edit to save',
  );
  expect(fixture.writes).toHaveLength(1);
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await expect(title).toBeEnabled();
  await expect(retry).toBeEnabled();
  await retry.click();
  await expect(page.locator('.document-workspace__save-status')).toHaveText('Saved');
  expect(fixture.writes).toHaveLength(2);
  expect(fixture.writes[1]).toEqual(original);
  await expect(title).toHaveValue('Unconfirmed title');
  await expect(title).toBeEditable();
  expect(fixture.boundary.errors).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
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

for (const width of [1600, 390]) {
  for (const reduced of [false, true]) {
    test(`LOCAL outline scrolling stays below toolbar at ${width}px reduced=${reduced}`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalDocuments();
      fixture.markdown =
        '# Product brief\n\n' +
        'Long-form notes for the upcoming release.\n\n'.repeat(35) +
        '## Next steps\n\n' +
        'Additional context.\n\n'.repeat(20);
      await page.setViewportSize({ width, height: 1000 });
      await page.emulateMedia({ reducedMotion: reduced ? 'reduce' : 'no-preference' });
      await page.addInitScript(() => {
        const scroll = Element.prototype.scrollIntoView;
        Element.prototype.scrollIntoView = function (options) {
          if (this.matches('.document-rich-text h2'))
            document.documentElement.dataset.outlineScroll =
              typeof options === 'object' ? options.behavior : 'unknown';
          scroll.call(this, options);
        };
      });
      await fixture.route(page, baseURL!);
      await page.goto(`/artifacts/${documentId}`);
      const content = page.getByRole('textbox', { name: 'Document content', exact: true });
      await expect(content).toBeVisible();
      await page.getByRole('button', { name: 'On this page', exact: true }).click();
      const outline = page.getByRole('navigation', { name: 'On this page' });
      const canvas = page.locator('.document-workspace__canvas');
      await canvas.evaluate((node) => {
        node.addEventListener('scrollend', () => node.setAttribute('data-scroll-settled', 'true'), {
          once: true,
        });
      });
      await outline.getByRole('button', { name: 'Next steps', exact: true }).click();
      await expect(canvas).toHaveAttribute('data-scroll-settled', 'true');
      const heading = content.getByRole('heading', { name: 'Next steps', exact: true });
      await expect(heading).toBeInViewport();
      await expect
        .poll(async () => {
          const headingBox = await heading.boundingBox();
          const toolbarBox = await page
            .getByRole('toolbar', { name: 'Document formatting' })
            .boundingBox();
          return headingBox!.y - (toolbarBox!.y + toolbarBox!.height);
        })
        .toBeGreaterThanOrEqual(0);
      await expect(page.locator('html')).toHaveAttribute(
        'data-outline-scroll',
        reduced ? 'instant' : 'smooth',
      );
      await page.screenshot({
        animations: 'disabled',
        path: info.outputPath(`outline-${width}-${reduced ? 'reduced' : 'smooth'}.png`),
      });
    });
  }
}

test('LOCAL Mermaid follows appearance changes without rewriting source', async ({
  page,
  baseURL,
}, info) => {
  const fixture = new LocalDocuments();
  await page.setViewportSize({ width: 390, height: 1000 });
  await page.addInitScript(() => localStorage.setItem('cma:appearance', 'light'));
  await fixture.route(page, baseURL!);
  await page.goto(`/artifacts/${documentId}`);
  const diagram = page.getByRole('img', { name: 'Mermaid diagram', exact: true });
  await expect(diagram).toBeVisible();
  const light = await diagram.getAttribute('src');
  await page.evaluate(() => document.body.setAttribute('data-ds-dark-theme', ''));
  await expect(diagram).toBeVisible();
  await expect.poll(() => diagram.getAttribute('src')).not.toBe(light);
  const dark = await diagram.getAttribute('src');
  const connector = await page.evaluate(() =>
    getComputedStyle(document.body).getPropertyValue('--dsw-alias-label-secondary').trim(),
  );
  expect(decodeURIComponent(dark!)).toContain(connector);
  await diagram.scrollIntoViewIfNeeded();
  await page.screenshot({
    animations: 'disabled',
    path: info.outputPath('diagram-390-dark-switched.png'),
  });
  await page.evaluate(() => document.body.removeAttribute('data-ds-dark-theme'));
  await expect(diagram).toBeVisible();
  await expect.poll(() => diagram.getAttribute('src')).not.toBe(dark);
  expect(fixture.writes).toHaveLength(0);
  expect(fixture.boundary.errors).toEqual([]);
});
