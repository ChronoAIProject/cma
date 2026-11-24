import { test, expect, type Page } from '@playwright/test';
import { LocalDocuments, documentId } from './documents.fixture';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { registerDocumentRedesignCases } from './redesign-document.cases';

registerDocumentRedesignCases();

test('LOCAL unsaved title uses native refresh and tab-close confirmation after user input', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  await fixture.route(page, baseURL!);
  await page.goto(`/artifacts/${documentId}`);
  const title = page.getByRole('textbox', { name: 'Document title' });
  await title.fill('Unsaved title');
  await expect(page.locator('.document-workspace__save-status')).toHaveText('Unsaved changes');
  const refreshDialog = page.waitForEvent('dialog');
  const refresh = page.reload({ timeout: 2_000 }).catch(() => undefined);
  const dialog = await refreshDialog;
  expect(dialog.type()).toBe('beforeunload');
  await dialog.dismiss();
  await refresh;
  await expect(title).toHaveValue('Unsaved title');
  const closeDialog = page.waitForEvent('dialog');
  await page.close({ runBeforeUnload: true });
  const closing = await closeDialog;
  expect(closing.type()).toBe('beforeunload');
  await closing.dismiss();
  await expect(title).toHaveValue('Unsaved title');
  expect(page.isClosed()).toBe(false);
  expect(fixture.writes).toEqual([]);
  expect(fixture.boundary.errors).toEqual([]);
  const closed = page.waitForEvent('close');
  const finalDialog = page.waitForEvent('dialog');
  await page.close({ runBeforeUnload: true });
  await (await finalDialog).accept();
  await closed;
});

async function openDocument(page: Page, baseURL: string) {
  const fixture = new LocalDocuments();
  fixture.markdown = '# Saved body';
  await fixture.route(page, baseURL);
  await page.goto('/artifacts');
  await page.getByRole('link', { name: /Product brief/ }).click();
  await expect(page.getByRole('textbox', { name: 'Document content', exact: true })).toBeVisible();
  return fixture;
}
const titleOf = (page: Page) => page.getByRole('textbox', { name: 'Document title' });
const editorOf = (page: Page) =>
  page.getByRole('textbox', { name: 'Document content', exact: true });
const confirm = (page: Page) => page.getByRole('dialog', { name: 'Leave without saving?' });
const stay = (page: Page) =>
  confirm(page).getByRole('button', { name: 'Stay', exact: true }).click();
const leave = (page: Page) =>
  confirm(page).getByRole('button', { name: 'Leave without saving', exact: true }).click();

for (const change of ['title', 'body', 'both']) {
  test(`LOCAL unsaved ${change} cancels Back and sidebar navigation without replacing the editor`, async ({
    page,
    baseURL,
  }) => {
    const fixture = await openDocument(page, baseURL!);
    const editor = editorOf(page);
    const original = await editor.elementHandle();
    if (change !== 'body') await titleOf(page).fill('Retained title');
    if (change !== 'title') {
      await editor.click();
      await editor.press('ControlOrMeta+End');
      await page.keyboard.type(' retained body');
    }
    const expectedTitle = await titleOf(page).inputValue();
    const expectedBody = await editor.textContent();
    const url = page.url();
    const history = await page.evaluate(() => ({
      state: window.history.state,
      length: window.history.length,
    }));
    await page.getByRole('button', { name: 'Back to artifacts' }).click();
    await stay(page);
    await expect(titleOf(page)).toHaveValue(expectedTitle);
    await expect(editor).toHaveText(expectedBody!);
    expect(await editor.evaluate((node, original) => node === original, original)).toBe(true);
    expect(page.url()).toBe(url);
    expect(
      await page.evaluate(() => ({ state: window.history.state, length: window.history.length })),
    ).toEqual(history);
    await page.getByRole('button', { name: 'Preview', exact: true }).click();
    await expect(titleOf(page)).toBeDisabled();
    await expect(page.getByRole('button', { name: 'Save', exact: true })).toBeDisabled();
    await page.getByRole('link', { name: 'Artifacts', exact: true }).click();
    await stay(page);
    expect(await editor.evaluate((node, original) => node === original, original)).toBe(true);
    expect(fixture.writes).toEqual([]);
    await page.getByRole('button', { name: 'Back to artifacts' }).click();
    await leave(page);
    await expect(page).toHaveURL(/\/artifacts$/);
    expect(await page.evaluate(() => window.history.length)).toBe(history.length + 1);
    expect(fixture.writes).toEqual([]);
    expect(fixture.boundary.errors).toEqual([]);
  });
}

test('LOCAL unsaved browser Back and Forward restore the same history entry on Stay', async ({
  page,
  baseURL,
}) => {
  const fixture = await openDocument(page, baseURL!);
  await titleOf(page).fill('History draft');
  const original = await editorOf(page).elementHandle();
  const source = await page.evaluate(() => window.history.state);
  await page.evaluate(() => window.history.back());
  await expect(confirm(page)).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/artifacts/${documentId}$`));
  expect(await page.evaluate(() => window.history.state)).toEqual(source);
  await stay(page);
  expect(await editorOf(page).evaluate((node, original) => node === original, original)).toBe(true);
  await page.evaluate(() => window.history.back());
  await leave(page);
  await expect(page).toHaveURL(/\/artifacts$/);
  await page.evaluate(() => window.history.forward());
  await expect(titleOf(page)).toHaveValue('History draft');
  await page.getByRole('button', { name: 'Back to artifacts' }).click();
  await leave(page);
  await page.evaluate(() => window.history.back());
  await expect(titleOf(page)).toHaveValue('History draft');
  await page.evaluate(() => window.history.forward());
  await expect(confirm(page)).toBeVisible();
  await stay(page);
  await expect(page).toHaveURL(new RegExp(`/artifacts/${documentId}$`));
  await page.evaluate(() => window.history.forward());
  await leave(page);
  await expect(page).toHaveURL(/\/artifacts$/);
  expect(fixture.writes).toEqual([]);
});

test('LOCAL unsaved pending save survives Stay and clears protection only after acknowledgement', async ({
  page,
  baseURL,
}) => {
  const fixture = await openDocument(page, baseURL!);
  let finish!: () => void;
  fixture.saveGate = new Promise<void>((resolve) => {
    finish = resolve;
  });
  await titleOf(page).fill('Acknowledged title');
  await page.getByRole('button', { name: 'Save', exact: true }).click();
  await expect.poll(() => fixture.writes.length).toBe(1);
  await page.getByRole('button', { name: 'Back to artifacts' }).click();
  await expect(confirm(page)).toContainText('Your save has not been confirmed');
  await stay(page);
  await expect(titleOf(page)).toHaveValue('Acknowledged title');
  finish();
  await expect(page.locator('.document-workspace__save-status')).toHaveText('Saved');
  await page.getByRole('button', { name: 'Back to artifacts' }).click();
  await expect(page).toHaveURL(/\/artifacts$/);
  expect(fixture.writes).toHaveLength(1);
  expect(fixture.writes[0]?.key).toBeTruthy();
});

test('LOCAL unsaved drafts stay isolated between documents and a clean document leaves without confirmation', async ({
  page,
  baseURL,
}) => {
  const fixture = await openDocument(page, baseURL!);
  await titleOf(page).fill('First document draft');
  await page.getByRole('button', { name: 'Back to artifacts' }).click();
  await leave(page);
  fixture.document = { ...fixture.document, id: `doc_${'b'.repeat(32)}`, title: 'Second document' };
  await page.getByRole('button', { name: 'Yours', exact: true }).click();
  await page.getByRole('link', { name: /Second document/ }).click();
  await expect(titleOf(page)).toHaveValue('Second document');
  await titleOf(page).fill('Second document draft');
  await page.getByRole('button', { name: 'Back to artifacts' }).click();
  await stay(page);
  await expect(titleOf(page)).toHaveValue('Second document draft');
  await titleOf(page).fill('Second document');
  await page.getByRole('button', { name: 'Back to artifacts' }).click();
  await expect(page).toHaveURL(/\/artifacts$/);
  fixture.document = { ...fixture.document, id: documentId, title: 'Product brief' };
  await page.getByRole('button', { name: 'Yours', exact: true }).click();
  await page.getByRole('link', { name: /Product brief/ }).click();
  await expect(titleOf(page)).toHaveValue('First document draft');
  expect(fixture.writes).toEqual([]);
});

test('LOCAL unsaved document survives Usage and same-owner session renewal without prompting', async ({
  page,
  baseURL,
}) => {
  const fixture = await openDocument(page, baseURL!);
  const metrics = new LocalUsageMetrics();
  await metrics.route(page);
  await titleOf(page).fill('Usage draft');
  const editor = await editorOf(page).elementHandle();
  await page.evaluate(() => window.dispatchEvent(new Event('focus')));
  await expect(titleOf(page)).toHaveValue('Usage draft');
  await page.getByRole('button', { name: /Account menu for/ }).click();
  await page.getByRole('menuitem', { name: 'Usage', exact: true }).click();
  const usage = page.getByRole('dialog', { name: 'Your usage' });
  await expect(usage).toBeVisible();
  expect(await page.evaluate(() => window.history.state.usageBackground)).toBe(
    `/artifacts/${documentId}`,
  );
  await expect(confirm(page)).toHaveCount(0);
  await usage.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(titleOf(page)).toHaveValue('Usage draft');
  expect(await editorOf(page).evaluate((node, original) => node === original, editor)).toBe(true);
  await page.getByRole('button', { name: 'Back to artifacts' }).click();
  await stay(page);
  expect(fixture.writes).toEqual([]);
});

test('LOCAL unsaved Back restores an older entry without the app history marker', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  fixture.markdown = '# Saved body';
  await fixture.route(page, baseURL!);
  await page.goto('/artifacts');
  await expect(page.getByRole('link', { name: /Product brief/ })).toBeVisible();
  await page.evaluate(() => window.history.replaceState({ legacy: 'preserved' }, '', '/artifacts'));
  await page.getByRole('link', { name: /Product brief/ }).click();
  await titleOf(page).fill('Legacy history draft');
  await page.evaluate(() => window.history.back());
  await expect(confirm(page)).toBeVisible();
  await expect(page).toHaveURL(new RegExp(`/artifacts/${documentId}$`));
  await stay(page);
  await expect(titleOf(page)).toHaveValue('Legacy history draft');
  await page.evaluate(() => window.history.back());
  await leave(page);
  await expect(page).toHaveURL(/\/artifacts$/);
  expect(await page.evaluate(() => window.history.state.legacy)).toBe('preserved');
  expect(fixture.writes).toEqual([]);
});
