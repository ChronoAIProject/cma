import { expect, test, type Locator } from '@playwright/test';
import { LocalDocuments, documentId } from './documents.fixture';
import type { Editor } from '@tiptap/core';

const url = 'https://example.test/en/about/previous-releases';
const suffix = '；访问 UTC：2026';
const imported = `Node URL：${url}${suffix}`;
const typed = 'https://example.test/typed/?q=one#results';
const typedSuffix = '」后文';
const pasted = 'https://example.test/pasted%EF%BC%9B';
const explicit = 'https://example.test/合法中文路径；访问';

async function expectLinks(content: Locator, hrefs: string[]) {
  await expect(content.locator('a')).toHaveCount(hrefs.length);
  expect(
    await content.locator('a').evaluateAll((anchors) =>
      anchors.map((node) => ({
        attribute: node.getAttribute('href'),
        href: (node as HTMLAnchorElement).href,
      })),
    ),
  ).toEqual(hrefs.map((href) => ({ attribute: href, href: new URL(href).href })));
  for (const link of await content.locator('a').all()) {
    await expect(link).toHaveAttribute('rel', 'noopener noreferrer');
    await expect(link).toHaveAttribute('target', '_blank');
  }
}

test('LOCAL CJK delimiters preserve link targets in Edit and Preview', async ({
  page,
  baseURL,
  context,
}) => {
  const fixture = new LocalDocuments();
  const paragraphs = [`${url}：访问`, `「${typed}${typedSuffix}`];
  fixture.markdown = paragraphs.join('\n\n');
  await fixture.route(page, baseURL!);
  await context.route('https://example.test/**', (route) =>
    route.fulfill({ status: 200, body: 'Synthetic link destination' }),
  );
  await page.goto(`/artifacts/${documentId}`);
  const content = page.getByRole('textbox', { name: 'Document content', exact: true });
  await expectLinks(content, [url, typed]);
  await expect(content.locator('p')).toHaveText(paragraphs);
  await page.getByRole('button', { name: 'Preview', exact: true }).click();
  await expectLinks(content, [url, typed]);
  await expect(content.locator('p')).toHaveText(paragraphs);
  const opened = page.waitForEvent('popup');
  await content.getByRole('link', { name: typed, exact: true }).click();
  const popup = await opened;
  await expect(popup).toHaveURL(typed);
  await popup.close();
  await page.reload();
  await expectLinks(content, [url, typed]);
  expect(fixture.writes).toHaveLength(0);
});

test('LOCAL autolink bounds import, typing and paste through Preview, save and reopen', async ({
  page,
  baseURL,
  context,
}) => {
  const fixture = new LocalDocuments();
  fixture.markdown = `${imported}\n\n[说明](${explicit})`;
  await fixture.route(page, baseURL!);
  const external: string[] = [];
  await context.route('https://example.test/**', async (route) => {
    external.push(route.request().url());
    await route.fulfill({
      status: 200,
      contentType: 'text/plain',
      body: 'Synthetic link destination',
    });
  });
  await page.goto(`/artifacts/${documentId}`);
  const content = page.getByRole('textbox', { name: 'Document content', exact: true });
  const status = page.locator('.document-workspace__save-status');
  await expectLinks(content, [url, explicit]);
  await expect(content.locator('p').first()).toHaveText(imported);
  await expect(content.locator('a').first()).toHaveText(url);
  expect(
    await content
      .locator('a')
      .first()
      .evaluate((node) => node.nextSibling?.textContent),
  ).toBe(suffix);
  await page.getByRole('button', { name: 'Preview', exact: true }).click();
  await expect(content).toHaveAttribute('contenteditable', 'false');
  await expect(page.getByRole('textbox', { name: 'Document title' })).toBeDisabled();
  await expect(page.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
  await expect(status).toHaveText('Saved');
  expect(fixture.writes).toHaveLength(0);
  const popupPromise = page.waitForEvent('popup');
  await content.locator('a').first().click();
  const popup = await popupPromise;
  await expect.poll(() => external).toEqual([url]);
  await expect.poll(() => popup.url()).toBe(url);
  await popup.close();
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await expect(content).toHaveAttribute('contenteditable', 'true');
  await content.evaluate((node) => {
    const editor = (node as HTMLElement & { editor: Editor }).editor;
    editor.commands.setTextSelection(editor.state.doc.content.size - 1);
    editor.commands.focus();
  });
  await expect
    .poll(() =>
      content.evaluate((node) => {
        const editor = (node as HTMLElement & { editor: Editor }).editor;
        return editor.state.selection.from === editor.state.doc.content.size - 1;
      }),
    )
    .toBe(true);
  await content.press('Enter');
  await page.keyboard.type(typed);
  await page.keyboard.insertText(typedSuffix);
  await expectLinks(content, [url, explicit, typed]);
  await content.press('Enter');
  await content.evaluate((node, text) => {
    const data = new DataTransfer();
    data.setData('text/plain', text);
    node.dispatchEvent(
      new ClipboardEvent('paste', { bubbles: true, cancelable: true, clipboardData: data }),
    );
  }, `${pasted}。后文`);
  await expectLinks(content, [url, explicit, typed, pasted]);
  await expect(content.locator('p').last()).toHaveText(`${pasted}。后文`);
  await content.press('ControlOrMeta+a');
  const copied = await content.evaluate((node) => {
    const data = new DataTransfer();
    node.dispatchEvent(
      new ClipboardEvent('copy', { bubbles: true, cancelable: true, clipboardData: data }),
    );
    return data.getData('text/plain');
  });
  expect(copied).toContain(imported);
  expect(copied).toContain(`${typed}${typedSuffix}`);
  expect(copied).toContain(`${pasted}。后文`);
  expect(
    await content
      .locator('a')
      .last()
      .evaluate((node) => node.nextSibling?.textContent),
  ).toBe('。后文');
  await page.getByRole('button', { name: 'Preview', exact: true }).click();
  await expectLinks(content, [url, explicit, typed, pasted]);
  await expect(status).toHaveText('Unsaved changes · Switch to Edit to save');
  expect(fixture.writes).toHaveLength(0);
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect(status).toHaveText('Saved');
  expect(fixture.writes).toHaveLength(1);
  expect(fixture.markdown).toContain(`[${url}](${url})${suffix}`);
  expect(fixture.markdown).toContain(`[${typed}](${typed})${typedSuffix}`);
  expect(fixture.markdown).toContain(`[${pasted}](${pasted})。后文`);
  expect(fixture.markdown).toContain(`[说明](${explicit})`);
  await page.reload();
  await expectLinks(content, [url, explicit, typed, pasted]);
  await expect(status).toHaveText('Saved');
  await page.getByRole('button', { name: 'Back to artifacts' }).click();
  await page.getByRole('link', { name: /Product brief/ }).click();
  await expectLinks(content, [url, explicit, typed, pasted]);
  expect(fixture.writes).toHaveLength(1);
  expect(context.pages()).toHaveLength(1);
  expect(external).toEqual([url]);
  expect(fixture.boundary.errors).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
});

test('LOCAL readonly autolink import retains punctuation without saving', async ({
  page,
  baseURL,
  context,
}) => {
  const fixture = new LocalDocuments();
  fixture.markdown = imported;
  fixture.document.access = 'read';
  await fixture.route(page, baseURL!);
  await context.route('https://example.test/**', (route) => route.abort());
  await page.goto(`/artifacts/${documentId}`);
  const content = page.getByRole('textbox', { name: 'Document content', exact: true });
  await expect(content).toHaveAttribute('contenteditable', 'false');
  await expectLinks(content, [url]);
  await expect(content).toHaveText(imported);
  await expect(page.getByRole('button', { name: 'Save', exact: true })).toHaveCount(0);
  await content.focus();
  await page.keyboard.insertText('Unwanted edit');
  await page.keyboard.press('ControlOrMeta+s');
  await page.reload();
  await expectLinks(content, [url]);
  await expect(content).toHaveText(imported);
  expect(fixture.writes).toHaveLength(0);
  expect(fixture.boundary.errors).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
});
