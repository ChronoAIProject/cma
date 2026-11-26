import { test, expect, type Page } from '@playwright/test';
import { LocalDocuments, documentId } from './documents.fixture';

async function start(page: Page) {
  await page.goto('/artifacts');
  await page.getByRole('button', { name: 'New document', exact: true }).click();
  await page.getByRole('textbox', { name: 'Title', exact: true }).fill('Original private title');
  await page.getByRole('button', { name: 'Create document', exact: true }).click();
}
type Attempt = { key: string | undefined; body: unknown };

for (const status of [503, 408, 409]) {
  test(`LOCAL creation HTTP ${status} followed by refusal cannot unlock the original request`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalDocuments();
    await fixture.route(page, baseURL!);
    const attempts: Attempt[] = [];
    await page.route('**/api/v1/documents', async (route) => {
      const request = route.request();
      if (request.method() !== 'POST') return route.fallback();
      attempts.push({ key: request.headers()['idempotency-key'], body: request.postDataJSON() });
      if (attempts.length <= 2)
        return route.fulfill({
          status: attempts.length === 1 ? status : 403,
          json: { detail: 'Synthetic creation refusal' },
        });
      return route.fallback();
    });
    await start(page);
    const dialog = page.getByRole('dialog', { name: 'New document' });
    const retry = dialog.getByRole('button', { name: 'Retry creation', exact: true });
    await expect(retry).toBeEnabled();
    await expect(dialog.getByRole('button', { name: 'Edit as a new request' })).toHaveCount(0);
    await dialog.getByText('Get help with this request', { exact: true }).click();
    const details = dialog.getByLabel('Request help details');
    const identity = (await details.innerText()).split('\n').slice(0, 4).join('\n');
    expect(attempts[0]!.key).toBeTruthy();
    expect(identity).toContain(`Creation request reference: ${attempts[0]!.key}`);
    await retry.click();
    await expect(dialog.getByText('The original creation is still unconfirmed.')).toBeVisible();
    await expect(dialog.getByRole('textbox', { name: 'Title', exact: true })).toBeDisabled();
    await expect(dialog.getByRole('button', { name: 'Edit as a new request' })).toHaveCount(0);
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(dialog).toBeHidden();
    await page.getByRole('button', { name: 'Resume creation', exact: true }).click();
    await expect(dialog.getByText('The original creation is still unconfirmed.')).toBeVisible();
    await expect(dialog.getByRole('textbox', { name: 'Title', exact: true })).toHaveValue(
      'Original private title',
    );
    await expect(dialog.getByRole('textbox', { name: 'Title', exact: true })).toBeDisabled();
    await expect(dialog.getByRole('button', { name: 'Edit as a new request' })).toHaveCount(0);
    await dialog.getByText('Get help with this request', { exact: true }).click();
    await expect(details).toContainText(identity);
    await expect(details).toContainText('Last attempt: Refused\nHTTP status: 403');
    expect(attempts).toHaveLength(2);
    expect(fixture.writes).toEqual([]);
    await retry.click();
    await expect(page).toHaveURL(`/artifacts/${documentId}`);
    expect(attempts).toEqual([attempts[0], attempts[0], attempts[0]]);
    expect(fixture.writes).toHaveLength(1);
    expect(fixture.boundary.writes).toEqual([]);
    expect(fixture.boundary.errors).toEqual([]);
    expect(fixture.boundary.unexpected).toEqual([]);
  });
}

for (const mobile of [false, true]) {
  test(`LOCAL creation retains its request through help, cancel, reload and delayed retry (${mobile ? 'mobile' : 'desktop'})`, async ({
    page,
    baseURL,
  }) => {
    if (mobile) await page.setViewportSize({ width: 390, height: 844 });
    const fixture = new LocalDocuments();
    await fixture.route(page, baseURL!);
    const attempts: Attempt[] = [];
    let release!: () => void;
    const gate = new Promise<void>((resolve) => {
      release = resolve;
    });
    await page.route('**/api/v1/documents', async (route) => {
      const request = route.request();
      if (request.method() !== 'POST') return route.fallback();
      attempts.push({ key: request.headers()['idempotency-key'], body: request.postDataJSON() });
      if (attempts.length === 1)
        return route.fulfill({
          status: 503,
          json: {
            detail: 'Document storage is unavailable. synthetic-private-owner',
            code: 'synthetic-private-code',
          },
        });
      await gate;
      await route.fallback();
    });
    await start(page);
    const dialog = page.getByRole('dialog', { name: 'New document' });
    await expect(
      dialog.getByText('Creation has not been confirmed. Retry uses the same request.'),
    ).toBeVisible();
    const title = dialog.getByRole('textbox', { name: 'Title', exact: true });
    await expect(title).toHaveValue('Original private title');
    await expect(title).toBeDisabled();
    await expect(title).toHaveAccessibleDescription('The title stays with this request.');
    await expect(page).toHaveURL(/\/artifacts$/);
    await dialog.getByText('Get help with this request', { exact: true }).click();
    const details = dialog.getByLabel('Request help details');
    await expect(details).toBeVisible();
    const help = dialog.getByRole('link', { name: 'Open artifact help (new tab)' });
    await expect(help).toHaveAttribute('href', '/docs/find-your-outputs');
    const opened = page.waitForEvent('popup');
    await help.click();
    const helpPage = await opened;
    await expect(helpPage).toHaveURL(/\/docs\/find-your-outputs$/);
    await helpPage.close();
    await expect(title).toHaveValue('Original private title');
    expect(attempts).toHaveLength(1);
    await page.screenshot({ path: test.info().outputPath('creation-help.png') });
    const safeDetails = await details.innerText();
    expect(safeDetails).toContain(`Creation request reference: ${attempts[0]!.key}`);
    expect(safeDetails).toMatch(/Build: CMA .+\nRequest started \(UTC\): \d{4}-\d{2}-\d{2}T/);
    expect(safeDetails).toContain('HTTP status: 503');
    for (const excluded of [
      'Original private title',
      'synthetic-private-owner',
      'synthetic-private-code',
      'Document storage is unavailable',
      documentId,
      'local-owner',
    ])
      expect(safeDetails).not.toContain(excluded);
    await page.context().grantPermissions(['clipboard-read', 'clipboard-write']);
    await dialog.getByRole('button', { name: 'Copy request details' }).click();
    await expect(dialog.getByText('Request details copied.')).toBeVisible();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(safeDetails);
    await page.evaluate(() => {
      navigator.clipboard.writeText = async () => {
        throw new Error('Clipboard unavailable');
      };
    });
    await dialog.getByRole('button', { name: 'Copy request details' }).click();
    await expect(
      dialog.getByText('Could not copy. Select the details above and copy them manually.'),
    ).toBeVisible();
    await expect
      .poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth))
      .toBe(true);
    expect(await details.evaluate((node) => node.scrollWidth <= node.clientWidth)).toBe(true);
    await dialog.getByRole('button', { name: 'Cancel', exact: true }).click();
    await expect(dialog).toBeHidden();
    await expect(page.getByText('Creation request: Original private title')).toBeVisible();
    expect(attempts).toHaveLength(1);
    await page.reload();
    await expect(page.getByText('Creation request: Original private title')).toBeVisible();
    await page.getByRole('button', { name: 'Resume creation', exact: true }).click();
    await expect(title).toHaveValue('Original private title');
    expect(attempts).toHaveLength(1);
    await dialog.getByRole('button', { name: 'Retry creation', exact: true }).click();
    await expect(dialog.getByRole('button', { name: 'Creating…' })).toBeDisabled();
    await expect(dialog.getByRole('button', { name: 'Cancel', exact: true })).toBeDisabled();
    await page.keyboard.press('Escape');
    await expect(dialog).toBeVisible();
    await expect(page).toHaveURL(/\/artifacts$/);
    await expect.poll(() => attempts.length).toBe(2);
    expect(attempts[1]).toEqual(attempts[0]);
    release();
    await expect(page).toHaveURL(`/artifacts/${documentId}`);
    await expect(page.getByRole('textbox', { name: 'Document title', exact: true })).toHaveValue(
      'Original private title',
    );
    expect(attempts).toHaveLength(2);
    expect(fixture.writes).toHaveLength(1);
    expect(fixture.boundary.errors).toEqual([]);
  });
}

test('LOCAL malformed confirmation and a lost response keep the same creation request', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  await fixture.route(page, baseURL!);
  const attempts: Attempt[] = [];
  await page.route('**/api/v1/documents', async (route) => {
    const request = route.request();
    if (request.method() !== 'POST') return route.fallback();
    attempts.push({ key: request.headers()['idempotency-key'], body: request.postDataJSON() });
    if (attempts.length === 1)
      return route.fulfill({ status: 201, json: { title: 'Invalid confirmation' } });
    if (attempts.length === 2) return route.abort('failed');
    return route.fallback();
  });
  await start(page);
  const retry = page.getByRole('button', { name: 'Retry creation', exact: true });
  for (let failure = 0; failure < 2; failure++) {
    await expect(
      page.getByText('Creation has not been confirmed. Retry uses the same request.'),
    ).toBeVisible();
    await expect(page).toHaveURL(/\/artifacts$/);
    await expect(page.getByRole('textbox', { name: 'Title', exact: true })).toBeDisabled();
    await retry.click();
  }
  await expect(page).toHaveURL(`/artifacts/${documentId}`);
  expect(attempts).toHaveLength(3);
  expect(attempts[1]).toEqual(attempts[0]);
  expect(attempts[2]).toEqual(attempts[0]);
  expect(fixture.boundary.errors).toEqual([]);
});

test('LOCAL known refusal requires an explicit new request before editing its title', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  await fixture.route(page, baseURL!);
  const attempts: Attempt[] = [];
  await page.route('**/api/v1/documents', async (route) => {
    const request = route.request();
    if (request.method() !== 'POST') return route.fallback();
    attempts.push({ key: request.headers()['idempotency-key'], body: request.postDataJSON() });
    if (attempts.length <= 2)
      return route.fulfill({ status: 400, json: { detail: 'Invalid document request' } });
    return route.fallback();
  });
  await start(page);
  await expect(
    page.getByText('This attempt was refused (HTTP 400). Retry uses the same request.'),
  ).toBeVisible();
  await expect(
    page.getByText('Creation has not been confirmed. Retry uses the same request.'),
  ).toBeHidden();
  await page.getByRole('button', { name: 'Retry creation', exact: true }).click();
  await expect(page.getByText('Creation request refused', { exact: true })).toBeVisible();
  expect(attempts[1]).toEqual(attempts[0]);
  await page.getByRole('button', { name: 'Edit as a new request' }).click();
  expect(attempts).toHaveLength(2);
  await page.getByRole('textbox', { name: 'Title', exact: true }).fill('Explicit new title');
  await page.getByRole('button', { name: 'Create document', exact: true }).click();
  await expect(page).toHaveURL(`/artifacts/${documentId}`);
  expect(attempts[2]!.key).not.toBe(attempts[0]!.key);
  expect(attempts[2]!.body).toEqual({ title: 'Explicit new title', markdown: '# Untitled\n' });
  expect(fixture.boundary.errors).toEqual([]);
});
