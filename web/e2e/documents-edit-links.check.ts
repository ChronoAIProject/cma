import { expect, test, type BrowserContext, type Locator, type Page } from '@playwright/test';
import { LocalDocuments, documentId } from './documents.fixture';

const destination = 'https://example.test/target';
const markdown = `[Example](${destination}) and [**Nested**](${destination})\n\nEditable text.`;

async function trackNavigation(context: BrowserContext) {
  const requests: string[] = [];
  const pages: Page[] = [];
  context.on('page', (page) => pages.push(page));
  await context.route('https://example.test/**', async (route) => {
    requests.push(route.request().url());
    await route.fulfill({ contentType: 'text/plain', body: 'Synthetic link destination' });
  });
  return { requests, pages };
}

async function openLink(page: Page, link: Locator) {
  const opened = page.waitForEvent('popup');
  await link.click();
  const popup = await opened;
  await expect(popup).toHaveURL(destination);
  await popup.close();
}

// Separate physical clicks so ProseMirror cannot treat them as a double click.
async function independentClick(link: Locator, options?: Parameters<Locator['click']>[0]) {
  await new Promise((resolve) => setTimeout(resolve, 650));
  await link.click(options);
}

async function settleNavigation(page: Page) {
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  );
  await new Promise((resolve) => setTimeout(resolve, 150));
}

test('LOCAL Preview opens once and independent Edit clicks never navigate across mode switches', async ({
  page,
  context,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  fixture.markdown = markdown;
  await fixture.route(page, baseURL!);
  const navigation = await trackNavigation(context);
  await page.goto(`/artifacts/${documentId}`);
  const content = page.getByRole('textbox', { name: 'Document content', exact: true });
  const link = content.getByRole('link', { name: 'Example', exact: true });
  const status = page.locator('.document-workspace__save-status');
  const original = await content.elementHandle();
  for (let round = 1; round <= 2; round++) {
    await page.getByRole('button', { name: 'Preview', exact: true }).click();
    await expect(content).toHaveAttribute('contenteditable', 'false');
    await openLink(page, link);
    await expect.poll(() => navigation.requests).toEqual(Array(round).fill(destination));
    expect(navigation.pages).toHaveLength(round);
    await page.getByRole('button', { name: 'Edit', exact: true }).click();
    await expect(content).toHaveAttribute('contenteditable', 'true');
    expect(await content.evaluate((node, before) => node === before, original)).toBe(true);
    await independentClick(link);
    await settleNavigation(page);
    expect({ pages: navigation.pages.length, requests: navigation.requests }).toEqual({
      pages: round,
      requests: Array(round).fill(destination),
    });
    await expect(content).toHaveText('Example and NestedEditable text.');
    await expect(status).toHaveText('Saved');
    expect(fixture.writes).toHaveLength(0);
  }
  expect(context.pages()).toHaveLength(1);
  expect(fixture.boundary.errors).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
});

test('LOCAL initially Edit suppresses plain, nested, modifier and middle clicks without changing content', async ({
  page,
  context,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  fixture.markdown = markdown;
  await fixture.route(page, baseURL!);
  const navigation = await trackNavigation(context);
  await page.goto(`/artifacts/${documentId}`);
  const content = page.getByRole('textbox', { name: 'Document content', exact: true });
  await expect(content).toHaveAttribute('contenteditable', 'true');
  const link = content.getByRole('link', { name: 'Example', exact: true });
  for (const options of [
    {},
    { modifiers: ['Control'] },
    { modifiers: ['Meta'] },
    { modifiers: ['Shift'] },
    { modifiers: ['Alt'] },
    { button: 'middle' },
  ] satisfies Parameters<Locator['click']>[0][]) {
    await independentClick(link, options);
    await settleNavigation(page);
    expect(navigation.pages).toHaveLength(0);
    expect(navigation.requests).toEqual([]);
  }
  await independentClick(content.locator('a strong'));
  await settleNavigation(page);
  expect(navigation.pages).toHaveLength(0);
  expect(navigation.requests).toEqual([]);
  await expect(content).toHaveText('Example and NestedEditable text.');
  await expect(page.locator('.document-workspace__save-status')).toHaveText('Saved');
  expect(fixture.writes).toHaveLength(0);
  expect(fixture.boundary.errors).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
});

test('LOCAL link clicks preserve selection, typing, link copying, undo and explicit Save', async ({
  page,
  context,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  fixture.markdown = markdown;
  await fixture.route(page, baseURL!);
  const navigation = await trackNavigation(context);
  await page.goto(`/artifacts/${documentId}`);
  const content = page.getByRole('textbox', { name: 'Document content', exact: true });
  const status = page.locator('.document-workspace__save-status');
  const link = content.getByRole('link', { name: 'Example', exact: true });
  await independentClick(link);
  await expect(content).toBeFocused();
  expect(
    await link.evaluate((node) => node.contains(window.getSelection()?.anchorNode ?? null)),
  ).toBe(true);
  await page.keyboard.insertText('X');
  await expect(content.locator('a').first()).toContainText('X');
  await expect(status).toHaveText('Unsaved changes');
  expect(fixture.writes).toHaveLength(0);
  await page.getByRole('button', { name: 'Undo', exact: true }).click();
  await expect(content).toHaveText('Example and NestedEditable text.');
  await content.press('ControlOrMeta+a');
  const copied = await content.evaluate((node) => {
    const data = new DataTransfer();
    node.dispatchEvent(
      new ClipboardEvent('copy', { bubbles: true, cancelable: true, clipboardData: data }),
    );
    return { text: data.getData('text/plain'), html: data.getData('text/html') };
  });
  expect(copied.text).toContain('Example and Nested');
  expect(copied.html).toContain(`href="${destination}"`);
  await content.locator('p').last().click();
  await page.keyboard.press('End');
  await page.keyboard.insertText(' Draft');
  await page.getByRole('button', { name: 'Preview', exact: true }).click();
  await expect(status).toHaveText('Unsaved changes · Switch to Edit to save');
  await openLink(page, content.locator('a strong'));
  expect(fixture.writes).toHaveLength(0);
  await page.getByRole('button', { name: 'Edit', exact: true }).click();
  await content.focus();
  await page.keyboard.press('ControlOrMeta+s');
  await expect(status).toHaveText('Saved');
  expect(fixture.writes).toHaveLength(1);
  expect(fixture.markdown).toContain('Draft');
  expect(fixture.markdown).toContain(`[Example](${destination})`);
  expect(navigation.pages).toHaveLength(1);
  expect(navigation.requests).toEqual([destination]);
  expect(context.pages()).toHaveLength(1);
  expect(fixture.boundary.errors).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
});

test('LOCAL readonly primary clicks retain navigation and safe attributes without writes', async ({
  page,
  context,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  fixture.markdown = `${markdown}\n\n[Unsafe](javascript:alert)\n\n[Unicode](https://example.test/合法路径；访问)`;
  fixture.document.access = 'read';
  await fixture.route(page, baseURL!);
  const navigation = await trackNavigation(context);
  await page.goto(`/artifacts/${documentId}`);
  const content = page.getByRole('textbox', { name: 'Document content', exact: true });
  await expect(content).toHaveAttribute('contenteditable', 'false');
  await expect(page.getByRole('button', { name: 'Save', exact: true })).toHaveCount(0);
  await expect(content.locator('a[href^="javascript:"]')).toHaveCount(0);
  await expect(content.getByRole('link', { name: 'Unicode' })).toHaveAttribute(
    'href',
    'https://example.test/合法路径；访问',
  );
  for (const link of await content.locator('a').all()) {
    await expect(link).toHaveAttribute('rel', 'noopener noreferrer');
    await expect(link).toHaveAttribute('target', '_blank');
  }
  await openLink(page, content.getByRole('link', { name: 'Example', exact: true }));
  await expect.poll(() => navigation.requests).toEqual([destination]);
  expect(navigation.pages).toHaveLength(1);
  expect(context.pages()).toHaveLength(1);
  expect(fixture.writes).toHaveLength(0);
  expect(fixture.boundary.errors).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
});
