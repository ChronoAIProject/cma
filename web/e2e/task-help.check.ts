import { expect, test } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { LocalDocuments } from './documents.fixture';

const topics = [
  ['Start a task', 'start-a-task'],
  ['Environments and capacity', 'environments-and-capacity'],
  ['Recover and continue', 'recover-and-continue'],
  ['Find your outputs', 'find-your-outputs'],
] as const;

for (const theme of ['light', 'dark']) {
  for (const width of [390, 1440]) {
    test(`LOCAL task help keyboard routes and layout at ${width}px ${theme}`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalStartup();
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
      await fixture.route(page, baseURL!);
      await page.goto('/');
      const account = page.getByRole('button', { name: /Account menu for/ });
      if (width === 390) await page.getByRole('button', { name: 'Show navigation' }).click();
      await account.click();
      await page.getByRole('menuitem', { name: 'Docs', exact: true }).click();
      await expect(page).toHaveURL(/\/docs$/);
      await expect(page.getByRole('heading', { name: 'Docs', exact: true })).toBeFocused();
      const cards = page.locator('.task-docs__cards');
      await expect(cards.getByRole('heading')).toHaveText(topics.map(([title]) => title));
      const boxes = await cards.getByRole('link').evaluateAll((links) =>
        links.map((link) => ({
          x: link.getBoundingClientRect().x,
          y: link.getBoundingClientRect().y,
        })),
      );
      if (width === 390) expect(boxes[0]!.x).toBe(boxes[1]!.x);
      else expect(boxes[0]!.y).toBe(boxes[1]!.y);
      await page.screenshot({ path: info.outputPath(`docs-${width}-${theme}.png`) });
      for (const [index, [title, path]] of topics.entries()) {
        const link = cards.getByRole('link', { name: new RegExp(`^${title}`) });
        for (let step = 0; step <= index; step++) await page.keyboard.press('Tab');
        await expect(link).toBeFocused();
        expect(await link.evaluate((node) => getComputedStyle(node).boxShadow)).not.toBe('none');
        await page.keyboard.press('Enter');
        await expect(page).toHaveURL(new RegExp(`/docs/${path}$`));
        await expect(page.getByRole('heading', { name: title, exact: true })).toBeFocused();
        await expect(page.getByRole('article')).toContainText('What you can do next');
        expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
          true,
        );
        expect(
          await page
            .locator('.console__surface')
            .evaluate((node) => node.scrollWidth <= node.clientWidth),
        ).toBe(true);
        await page.screenshot({ path: info.outputPath(`${path}-${width}-${theme}.png`) });
        await page.goBack();
        await expect(page).toHaveURL(/\/docs$/);
        await expect(page.getByRole('heading', { name: 'Docs', exact: true })).toBeFocused();
      }
      const protocol = page.getByRole('link', { name: /Event Gateway Protocol/ });
      await expect(protocol).toHaveAttribute('href', '/docs/event-gateway');
      await protocol.click();
      await expect(
        page.getByRole('heading', { name: 'Event Gateway Protocol', exact: true }),
      ).toBeVisible();
      await page.goto('/developer');
      await expect(page).toHaveURL(/\/docs$/);
      await expect(cards.getByRole('link')).toHaveCount(4);
      expect(fixture.writes).toEqual([]);
      expect(fixture.errors).toEqual([]);
      expect(fixture.unexpected).toEqual([]);
    });
  }
}

test('LOCAL Home, full capacity and uncertain task help preserve the draft and original request', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalStartup();
  await fixture.route(page, baseURL!);
  await page.route('**/api/v2/ui/task-experience', (route) =>
    route.fulfill({
      json: {
        object: 'cma.ui.task_experience',
        schema_version: 1,
        account_id: 'local-owner',
        capabilities: {
          task_home_admission: true,
          desktop_enable: false,
          suggested_followup_admission: false,
          browser_takeover: false,
          replay_presentation: false,
        },
      },
    }),
  );
  let full = true;
  await page.route('**/api/v1/sandboxes/capacity', (route) =>
    route.fulfill({
      json: {
        resource: 'sandbox',
        limit: 1,
        live: full ? 1 : 0,
        available: full ? 0 : 1,
        counts_by_status: full ? { retiring: 1 } : {},
        observed_at: new Date().toISOString(),
      },
    }),
  );
  const writes: { key?: string; body: unknown }[] = [];
  await page.route('**/api/agent-manager/v1/agents', (route) => {
    writes.push({
      key: route.request().headers()['idempotency-key'],
      body: route.request().postDataJSON(),
    });
    return route.abort('failed');
  });
  await page.goto('/');
  const draft = page.getByLabel('Your task');
  await draft.fill('Keep this original task through help and recovery');
  await page.getByRole('link', { name: 'Help starting a task', exact: true }).click();
  await expect(page.getByRole('heading', { name: 'Start a task', exact: true })).toBeFocused();
  await page.goBack();
  await expect(draft).toHaveValue('Keep this original task through help and recovery');
  await expect(page.getByRole('region', { name: 'Sandbox capacity' })).toContainText(
    'Environment limit reached',
  );
  await page.getByRole('link', { name: 'Help starting a task', exact: true }).click();
  await page.getByRole('link', { name: 'Docs', exact: true }).click();
  await page.getByRole('link', { name: /^Environments and capacity/ }).click();
  await expect(
    page.getByRole('heading', { name: 'Environments and capacity', exact: true }),
  ).toBeFocused();
  await expect(page.getByRole('article')).toContainText('A pending deletion is not a free slot');
  await page.goBack();
  await page.goBack();
  await page.goBack();
  await expect(draft).toHaveValue('Keep this original task through help and recovery');
  expect(writes).toEqual([]);
  const refreshCapacity = page.getByRole('button', { name: 'Refresh Sandbox capacity' });
  await expect(refreshCapacity).toBeEnabled();
  full = false;
  await refreshCapacity.click();
  await page.getByRole('button', { name: 'Start task', exact: true }).click();
  const recover = page.getByRole('button', { name: 'Recover original task', exact: true });
  await expect(recover).toBeEnabled();
  expect(writes).toHaveLength(1);
  expect(writes[0]!.key).toBeTruthy();
  await page.getByRole('link', { name: 'Help recovering this request' }).click();
  await expect(
    page.getByRole('heading', { name: 'Recover and continue', exact: true }),
  ).toBeFocused();
  await page.goBack();
  await expect(recover).toBeEnabled();
  expect(writes).toHaveLength(1);
  await expect(page.getByRole('region', { name: 'Original request', exact: true })).toContainText(
    'Keep this original task through help and recovery',
  );
  await recover.click();
  await expect(recover).toBeEnabled();
  expect(writes).toHaveLength(2);
  expect(writes[1]).toEqual(writes[0]);
  expect(fixture.writes).toEqual([]);
  expect(fixture.unexpected).toEqual([]);
  expect(fixture.errors).toEqual([]);
});

test('LOCAL Artifacts links to output guidance without changing documents', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalDocuments();
  await fixture.route(page, baseURL!);
  await page.goto('/artifacts');
  await expect(page.getByRole('link', { name: /Product brief/ })).toBeVisible();
  await page.getByRole('link', { name: 'Help finding your outputs' }).click();
  await expect(page.getByRole('heading', { name: 'Find your outputs', exact: true })).toBeFocused();
  await expect(page.getByRole('article')).toContainText(
    'Registration alone does not guarantee a successful download',
  );
  await expect(page.getByRole('article')).toContainText(
    'unconfirmed save is not evidence of a saved revision',
  );
  await page.goBack();
  await expect(page.getByRole('link', { name: /Product brief/ })).toBeVisible();
  expect(fixture.writes).toEqual([]);
  expect(fixture.boundary.writes).toEqual([]);
  expect(fixture.boundary.errors).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
});
