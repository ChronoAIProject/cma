import { test, expect, type Dialog } from '@playwright/test';
import { LocalDocuments, documentId } from './documents.fixture';
import { routeHistoryBot } from './bot-history.fixture';

test('LOCAL legacy history without a native index preserves Stay, Back, Forward and every entry', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  await fixture.route(page, baseURL!);
  await page.addInitScript(() => Object.defineProperty(window, 'navigation', { value: undefined }));
  // Simulate a tab whose history was created before the guarded router was loaded.
  await page.route(`${baseURL}/docs`, (route) =>
    route.request().isNavigationRequest()
      ? route.fulfill({ contentType: 'text/html', body: '<p>Legacy entry</p>' })
      : route.fallback(),
  );
  const path = `/artifacts/${documentId}`;
  await page.goto('/docs');
  await page.evaluate((path) => {
    history.replaceState({ legacy: 'first' }, '', '/docs');
    history.pushState({ legacy: 'middle' }, '', '/artifacts');
    history.pushState(null, '', path);
  }, path);
  await page.reload();
  const title = page.getByRole('textbox', { name: 'Document title' });
  await title.fill('Preserve the real cursor');
  const original = await title.elementHandle();
  const initial = await page.evaluate(() => ({ state: history.state, length: history.length }));
  for (let attempt = 0; attempt < 2; attempt++) {
    let nativeCancellation: Promise<void> | undefined;
    const dismiss = (dialog: Dialog) => {
      expect(dialog.type()).toBe('beforeunload');
      nativeCancellation = dialog.dismiss();
    };
    page.once('dialog', dismiss);
    await page.evaluate(() => history.go(-2));
    const modal = page.getByRole('dialog', { name: 'Leave without saving?' });
    await expect.poll(async () => !!nativeCancellation || (await modal.isVisible())).toBe(true);
    if (nativeCancellation) await nativeCancellation;
    else {
      page.off('dialog', dismiss);
      await modal.getByRole('button', { name: 'Stay', exact: true }).click();
    }
    await expect(title).toHaveValue('Preserve the real cursor');
    expect(await title.evaluate((node, old) => node === old, original)).toBe(true);
    expect(await page.evaluate(() => ({ state: history.state, length: history.length }))).toEqual(
      initial,
    );
    await title.fill('Product brief');
    await page.evaluate(() => history.forward());
    // Forward must be a no-op at the original newest entry, not move to the middle slot.
    await page.waitForTimeout(100);
    await expect(page).toHaveURL(new RegExp(`${path}$`));
    await title.fill('Preserve the real cursor');
  }
  await title.fill('Product brief');
  await page.evaluate(() => history.go(-2));
  await expect(page).toHaveURL(/\/docs$/);
  expect(await page.evaluate(() => history.state)).toEqual({ legacy: 'first' });
  await page.evaluate(() => history.forward());
  await expect(page).toHaveURL(/\/artifacts$/);
  expect(await page.evaluate(() => history.state)).toEqual({ legacy: 'middle' });
  await page.evaluate(() => history.forward());
  await expect(title).toHaveValue('Product brief');
  expect(fixture.writes).toEqual([]);
  expect(fixture.boundary.errors).toEqual([]);
});

test('LOCAL a missing-index tab started between legacy entries cancels in both directions', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  await fixture.route(page, baseURL!);
  await page.addInitScript(() => Object.defineProperty(window, 'navigation', { value: undefined }));
  await page.route(`${baseURL}/docs`, (route) =>
    route.request().isNavigationRequest()
      ? route.fulfill({ contentType: 'text/html', body: '<p>Legacy entry</p>' })
      : route.fallback(),
  );
  const path = `/artifacts/${documentId}`;
  await page.goto('/docs');
  await page.evaluate((path) => {
    history.replaceState({ legacy: 'first' }, '', '/docs');
    history.pushState({ legacy: 'document' }, '', path);
    history.pushState({ legacy: 'last' }, '', '/artifacts');
    history.back();
  }, path);
  await page.waitForURL(new RegExp(`${path}$`));
  const length = await page.evaluate(() => history.length);
  await page.reload();
  const title = page.getByRole('textbox', { name: 'Document title' });
  await title.fill('Keep the middle entry');
  const original = await title.elementHandle();
  const state = await page.evaluate(() => history.state);
  expect(state.legacy).toBe('document');
  expect(await page.evaluate(() => history.length)).toBe(length);
  for (const direction of [-1, 1, -1, 1]) {
    const confirmation = page.waitForEvent('dialog');
    await page.evaluate((delta) => history.go(delta), direction);
    const dialog = await confirmation;
    expect(dialog.type()).toBe('beforeunload');
    await dialog.dismiss();
    await expect(title).toHaveValue('Keep the middle entry');
    expect(await title.evaluate((node, old) => node === old, original)).toBe(true);
    expect(await page.evaluate(() => history.state)).toEqual(state);
    await expect(page).toHaveURL(new RegExp(`${path}$`));
  }
  await title.fill('Product brief');
  await page.evaluate(() => history.back());
  await expect(page).toHaveURL(/\/docs$/);
  expect(await page.evaluate(() => history.state.legacy)).toBe('first');
  await page.evaluate(() => history.forward());
  await expect(title).toHaveValue('Product brief');
  await title.fill('Explicitly discarded');
  const confirmation = page.waitForEvent('dialog');
  await page.evaluate(() => history.forward());
  await (await confirmation).accept();
  await expect(page).toHaveURL(/\/artifacts$/);
  await expect(page.getByRole('heading', { name: 'Artifacts', exact: true })).toBeVisible();
  expect(await page.evaluate(() => history.state.legacy)).toBe('last');
  expect(await page.evaluate(() => history.length)).toBe(length);
  expect(fixture.writes).toEqual([]);
  expect(fixture.boundary.errors).toEqual([]);
});

test('LOCAL app-indexed Back and Forward still use Stay without a native index', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  await fixture.route(page, baseURL!);
  await page.addInitScript(() => Object.defineProperty(window, 'navigation', { value: undefined }));
  await page.goto('/artifacts');
  await page.getByRole('link', { name: /Product brief/ }).click();
  const title = page.getByRole('textbox', { name: 'Document title' });
  await title.fill('Indexed draft');
  const state = await page.evaluate(() => history.state);
  const modal = page.getByRole('dialog', { name: 'Leave without saving?' });
  for (let attempt = 0; attempt < 2; attempt++) {
    await page.evaluate(() => history.back());
    await modal.getByRole('button', { name: 'Stay', exact: true }).click();
    await expect(title).toHaveValue('Indexed draft');
    expect(await page.evaluate(() => history.state)).toEqual(state);
  }
  await page.evaluate(() => history.back());
  await modal.getByRole('button', { name: 'Leave without saving', exact: true }).click();
  await expect(page).toHaveURL(/\/artifacts$/);
  await page.evaluate(() => history.forward());
  await expect(title).toHaveValue('Indexed draft');
  expect(fixture.writes).toEqual([]);
});

test('LOCAL successful Bot creation preserves dirty-document history without a native index', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  await fixture.route(page, baseURL!);
  const creations = await routeHistoryBot(page);
  await page.addInitScript(() => Object.defineProperty(window, 'navigation', { value: undefined }));
  await page.goto('/bots');
  await expect(page.getByRole('region', { name: 'Consumer workspace', exact: true })).toBeVisible();
  const entry = () => page.evaluate(() => ({ path: location.pathname, state: history.state }));
  const entries = [await entry()];
  await page
    .getByRole('region', { name: 'Consumer workspace', exact: true })
    .getByRole('button', { name: 'New consumer', exact: true })
    .click();
  const wizard = page.getByRole('region', { name: 'New consumer', exact: true });
  await wizard.getByLabel('Consumer name', { exact: true }).fill('History consumer');
  await page.evaluate(() =>
    history.replaceState({ ...history.state, unrelated: { retained: true } }, ''),
  );
  entries.push(await entry());
  await wizard.getByRole('button', { name: 'Event Gateway connection', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Research channel', exact: true }).click();
  await wizard.getByRole('button', { name: 'Continue', exact: true }).click();
  await wizard.getByRole('button', { name: 'Continue', exact: true }).click();
  await wizard.getByRole('button', { name: 'Admission policy', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Private chats only', exact: true }).click();
  await wizard.getByRole('button', { name: 'Continue', exact: true }).click();
  await wizard.getByRole('button', { name: 'Create consumer', exact: true }).click();
  await expect(page).toHaveURL(/\/bots\/bot_history$/);
  await expect(page.getByRole('heading', { name: 'History consumer', exact: true })).toBeVisible();
  entries.push(await entry());
  await page.evaluate(() =>
    window.dispatchEvent(
      new CustomEvent('cma:route-request', { detail: '/artifacts', cancelable: true }),
    ),
  );
  await expect(page.getByRole('link', { name: /Product brief/ })).toBeVisible();
  entries.push(await entry());
  await page.getByRole('link', { name: /Product brief/ }).click();
  const title = page.getByRole('textbox', { name: 'Document title' });
  await title.fill('Draft after creation');
  entries.push(await entry());
  const original = await title.elementHandle();
  const length = await page.evaluate(() => history.length);
  const modal = page.getByRole('dialog', { name: 'Leave without saving?' });
  for (let attempt = 0; attempt < 2; attempt++) {
    await page.evaluate(() => history.go(-4));
    await modal.getByRole('button', { name: 'Stay', exact: true }).click();
    await expect(title).toHaveValue('Draft after creation');
    expect(await title.evaluate((node, old) => node === old, original)).toBe(true);
    expect(await entry()).toEqual(entries[4]);
    await title.fill('Product brief');
    await page.evaluate(() => history.forward());
    // With no dirty guard to hide a misplaced cursor, Forward at the newest entry is a no-op.
    await page.waitForTimeout(100);
    expect(await entry()).toEqual(entries[4]);
    await title.fill('Draft after creation');
  }
  await page.evaluate(() => history.go(-4));
  await modal.getByRole('button', { name: 'Leave without saving', exact: true }).click();
  await expect(page).toHaveURL(/\/bots$/);
  expect(await entry()).toEqual(entries[0]);
  for (const expected of entries.slice(1)) {
    await page.evaluate(() => history.forward());
    await expect.poll(entry).toEqual(expected);
  }
  await expect(title).toHaveValue('Draft after creation');
  expect(await page.evaluate(() => history.length)).toBe(length);
  await title.fill('Product brief');
  await page.getByRole('button', { name: 'Back to artifacts' }).click();
  const last = await entry();
  await page.evaluate(() => history.back());
  await title.fill('Forward draft');
  for (let attempt = 0; attempt < 2; attempt++) {
    await page.evaluate(() => history.forward());
    await modal.getByRole('button', { name: 'Stay', exact: true }).click();
    await expect(title).toHaveValue('Forward draft');
    expect(await entry()).toEqual(entries[4]);
  }
  await page.evaluate(() => history.forward());
  await modal.getByRole('button', { name: 'Leave without saving', exact: true }).click();
  await expect.poll(entry).toEqual(last);
  expect(await page.evaluate(() => history.length)).toBe(length + 1);
  expect(creations).toEqual(['/api/v1/bots']);
  expect(fixture.writes).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
  expect(fixture.boundary.errors).toEqual([]);
});
