import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

test('LOCAL consumer dashboard uses real CMA, Gateway and Mongo for creation, test and cleanup', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  await page.route('**/auth/session', (route) =>
    route.fulfill({
      json: {
        authenticated: true,
        subject: 'owner',
        display_name: 'Local owner',
        capabilities: ['manage_own_bots'],
      },
    }),
  );
  await page.route('**/api/image-manager/v1/images?scope=all', (route) =>
    route.fulfill({ json: { items: [] } }),
  );
  await page.route('**/api/v1/bots**', async (route) => {
    const url = new URL(route.request().url());
    const upstream = await route.fetch({
      url: `${process.env.CMA_BOT_FIXTURE_ROOT}${url.pathname}${url.search}`,
      maxRedirects: 0,
      maxRetries: 0,
    });
    await route.fulfill({ response: upstream });
  });
  await page.setViewportSize({ width: 1440, height: 1000 });
  await page.goto('/bots');
  await page.getByRole('button', { name: 'New consumer', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Continue' })).toBeDisabled();
  await page.getByLabel('Consumer name', { exact: true }).fill('Browser acceptance Bot');
  await page.getByRole('button', { name: 'Event Gateway connection', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Research channel' }).click();
  await page.getByRole('button', { name: 'Continue' }).click();
  await expect(page.getByRole('heading', { name: 'Configure the Agent inline' })).toBeVisible();
  await expect(page.getByRole('radio', { name: /System default image/ })).toBeChecked();
  await page.getByLabel('Model', { exact: true }).fill('gpt-5');
  await page.getByRole('button', { name: 'Reasoning effort', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'high', exact: true }).click();
  await page.getByRole('button', { name: 'Permission level', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Guarded', exact: true }).click();
  await page.getByLabel('System prompt', { exact: true }).fill('Respond with concise updates.');
  await page.getByRole('button', { name: 'Continue' }).click();
  await page.getByRole('button', { name: 'Admission policy' }).click();
  await page.getByRole('menuitemradio', { name: 'Private chats only', exact: true }).click();
  await page.getByLabel('Acknowledgement delay (seconds)', { exact: true }).fill('0');
  await page.getByRole('button', { name: 'Continue' }).click();
  const creation = page.waitForRequest(
    (request) => request.method() === 'POST' && new URL(request.url()).pathname === '/api/v1/bots',
  );
  await page.getByRole('button', { name: 'Create consumer', exact: true }).click();
  const body: unknown = (await creation).postDataJSON();
  expect(body).toMatchObject({
    agent: {
      model: 'gpt-5',
      reasoning_effort: 'high',
      permission_mode: 'guarded',
      system_prompt: 'Respond with concise updates.',
      skills: [],
      permissions: { services: [] },
    },
  });
  expect(body).not.toHaveProperty('profile');
  await expect(page.getByText('Active', { exact: true })).toBeVisible();
  await expect(page.getByText('Event Gateway connection', { exact: true })).toBeVisible();
  for (const theme of ['light', 'dark']) {
    await page.evaluate(
      (value) => document.body.toggleAttribute('data-ds-dark-theme', value === 'dark'),
      theme,
    );
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      await expect
        .poll(() =>
          page.locator('.bots').evaluate((element) =>
            [...element.querySelectorAll('button,code')].every((control) => {
              const rect = control.getBoundingClientRect();
              return rect.width === 0 || (rect.left >= 0 && rect.right <= innerWidth + 1);
            }),
          ),
        )
        .toBe(true);
      await page.screenshot({
        path: info.outputPath(`bot-${theme}-${width}.png`),
        fullPage: true,
        animations: 'disabled',
      });
    }
  }
  await page.getByRole('button', { name: 'Run synthetic test' }).click();
  await expect(page.getByText(/Accepted after \d+ ms/)).toBeVisible();
  await page.getByRole('button', { name: 'Disable consumer', exact: true }).click();
  await expect(page.getByText('Disabled', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Enable consumer', exact: true }).click();
  await expect(page.getByText('Active', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Rotate keys', exact: true }).click();
  await page.getByRole('button', { name: 'Confirm key rotation' }).click();
  await expect(page.getByRole('button', { name: 'Delete consumer', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Delete consumer', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Confirm deletion' })).toBeDisabled();
  await page.getByRole('checkbox').check();
  await page.getByRole('button', { name: 'Confirm deletion' }).click();
  await expect(page.getByText(/Consumer deleted\./)).toBeVisible();
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
