import { test, expect, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

const ERROR = 'Enter a complete HTTP or HTTPS link to your spreadsheet.';
const LINK = ' https://example.com/synthetic.csv ';
const template = (link: string) => ({ id: 'spreadsheet', version: 1, fields: { link } });
const body = (link: string) => ({
  name: 'New task',
  template: template(link),
  initial_message: `Analyze the spreadsheet at ${link.trim()} and point out trends, outliers and what to do next.`,
});

async function chooseTemplate(page: Page, name: string) {
  const button = page.getByRole('button', { name, exact: true });
  const menu = page.getByRole('button', { name: 'Templates', exact: true });
  await expect(button.or(menu).first()).toBeVisible();
  if (await button.isVisible()) {
    await button.click();
    return;
  }
  await menu.click();
  await page.getByRole('menuitem', { name, exact: true }).click();
}

async function fixture(page: Page, baseURL: string) {
  const boundary = new LocalStartup();
  const writes: { body: unknown; key: string | undefined }[] = [];
  await boundary.route(page, baseURL);
  await page.route(`${baseURL}/api/v1/agents/agt_contract/reply-preview`, (route) => {
    if (route.request().method() !== 'GET') return route.fallback();
    return route.fulfill({
      json: {
        status: 'none',
        agent_id: 'agt_contract',
        source_through_seq: 0,
        latest_assistant_reply: null,
      },
    });
  });
  await page.route('**/openapi.json', (route) =>
    route.fulfill({
      json: {
        openapi: '3.1.0',
        info: { title: 'Synthetic', version: '1' },
        paths: {},
        'x-cma-task-templates-v1': true,
        components: {
          schemas: {
            AgentLifecycleCreateRequest: {
              properties: {
                initial_message: { type: 'string' },
              },
            },
          },
        },
      },
    }),
  );
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
  await page.route('**/api/agent-manager/v1/agents', (route) => {
    writes.push({
      body: route.request().postDataJSON(),
      key: route.request().headers()['idempotency-key'],
    });
    return route.abort('failed');
  });
  return { boundary, writes };
}

async function settle(page: Page) {
  await expect
    .poll(() =>
      page.locator('.frame__center').evaluate((node) => node.getBoundingClientRect().right),
    )
    .toBe(page.viewportSize()!.width);
  await page.evaluate(() => document.fonts.ready);
}

for (const theme of ['light', 'dark']) {
  for (const width of [1440, 390, 320]) {
    test(`LOCAL spreadsheet ${theme} ${width}px: invalid submissions cannot allocate; corrected request recovers exactly`, async ({
      page,
      baseURL,
    }, info) => {
      const { writes, boundary } = await fixture(page, baseURL!);
      await page.setViewportSize({ width, height: 900 });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
      await page.goto('/');
      await settle(page);
      await chooseTemplate(page, 'Analyze a spreadsheet');
      const input = page.getByLabel('Spreadsheet link', { exact: true });
      const start = page.getByRole('button', { name: 'Start task', exact: true });
      await input.fill('not-a-url');
      await expect(input).toHaveAccessibleDescription(ERROR);
      await expect(input).toHaveAttribute('aria-invalid', 'true');
      await expect(start).toBeDisabled();
      await start.evaluate((node: HTMLButtonElement) => node.click());
      await input.press('Enter');
      await input.press('Control+Enter');
      await input.press('Meta+Enter');
      await input.evaluate((node: HTMLInputElement) => {
        node.form!.requestSubmit();
        node.form!.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
      });
      await expect(input).toBeFocused();
      await expect(input).toHaveAccessibleDescription(ERROR);
      await expect(page.getByText(ERROR, { exact: true })).toHaveCount(1);
      expect(writes).toEqual([]);
      await input.fill(LINK);
      await expect(start).toBeEnabled();
      await expect(input).not.toHaveAttribute('aria-invalid');
      await expect(page.getByText(ERROR, { exact: true })).toHaveCount(0);
      await input.fill('https:///example.com');
      await expect(start).toBeDisabled();
      await expect(input).toHaveAccessibleDescription(ERROR);
      await expect
        .poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
        .toBe(true);
      for (const locator of [input, page.getByText(ERROR, { exact: true }), start]) {
        const bounds = await locator.boundingBox();
        expect(bounds).not.toBeNull();
        expect(bounds!.x).toBeGreaterThanOrEqual(0);
        expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width + 1);
      }
      await page.screenshot({
        path: info.outputPath(`spreadsheet-error-${theme}-${width}.png`),
        animations: 'disabled',
      });
      await input.fill(LINK);
      await input.press('Control+Enter');
      await expect(page.getByRole('button', { name: 'Recover original task' })).toBeVisible();
      expect(writes).toHaveLength(1);
      expect(writes[0]!.body).toEqual(body(LINK));
      expect(writes[0]!.key).toBeTruthy();
      const followup = page.getByRole('textbox', { name: 'Follow-up draft', exact: true });
      await followup.fill('a later invalid draft');
      await page.reload();
      await settle(page);
      await expect(followup).toHaveValue('a later invalid draft');
      await page.getByRole('button', { name: 'Recover original task' }).click();
      await expect.poll(() => writes.length).toBe(2);
      expect(writes[1]).toEqual(writes[0]);
      await expect(followup).toHaveValue('a later invalid draft');
      expect(boundary.writes).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.errors).toEqual([]);
    });
  }
}

test('LOCAL invalid spreadsheet drafts survive switching and reload without a request', async ({
  page,
  baseURL,
}) => {
  const { writes, boundary } = await fixture(page, baseURL!);
  await page.setViewportSize({ width: 390, height: 900 });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.goto('/');
  await settle(page);
  await chooseTemplate(page, 'Analyze a spreadsheet');
  await page.getByLabel('Spreadsheet link', { exact: true }).fill('not-a-url');
  await chooseTemplate(page, 'Write a document');
  await page.getByLabel('Subject', { exact: true }).fill('Separate synthetic draft');
  await chooseTemplate(page, 'Analyze a spreadsheet');
  await page.reload();
  await settle(page);
  await expect(page.getByLabel('Spreadsheet link', { exact: true })).toHaveValue('not-a-url');
  await expect(page.getByRole('button', { name: 'Start task', exact: true })).toBeDisabled();
  await chooseTemplate(page, 'Write a document');
  await expect(page.getByLabel('Subject', { exact: true })).toHaveValue('Separate synthetic draft');
  expect(writes).toEqual([]);
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});

test('LOCAL retained legacy invalid spreadsheet recovers its original request after reload', async ({
  page,
  baseURL,
}) => {
  const { writes, boundary } = await fixture(page, baseURL!);
  const retained = { key: 'legacy-spreadsheet-request', body: body('not-a-url') };
  await page.addInitScript((intent) => {
    const key = `cma:agent-creation:${JSON.stringify([location.origin, 'local-owner'])}`;
    if (sessionStorage.getItem(key) === null)
      sessionStorage.setItem(key, JSON.stringify({ version: 1, intent }));
  }, retained);
  await page.goto('/');
  await expect(page.getByRole('button', { name: 'Recover original task' })).toBeVisible();
  expect(writes).toEqual([]);
  await page.reload();
  await page.getByRole('button', { name: 'Recover original task' }).click();
  await expect.poll(() => writes.length).toBe(1);
  expect(writes[0]).toEqual(retained);
  expect(boundary.writes).toEqual([]);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
