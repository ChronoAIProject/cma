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
  const consumers = page
    .getByRole('navigation', { name: 'Event Gateway navigation', exact: true })
    .getByRole('region', { name: 'Consumers', exact: true });
  const welcome = page.getByRole('region', { name: 'Consumer workspace', exact: true });
  await welcome.getByRole('button', { name: 'New consumer', exact: true }).click();
  const wizard = page.getByRole('region', { name: 'New consumer', exact: true });
  const consumerName = 'Browser acceptance Bot';
  await expect(wizard.getByRole('button', { name: 'Continue', exact: true })).toBeDisabled();
  await wizard.getByLabel('Consumer name', { exact: true }).fill(consumerName);
  await wizard.getByRole('button', { name: 'Event Gateway connection', exact: true }).click();
  await page
    .getByRole('menu', { name: 'Event Gateway connection', exact: true })
    .getByRole('menuitemradio', { name: 'Research channel', exact: true })
    .click();
  await wizard.getByRole('button', { name: 'Continue', exact: true }).click();
  await expect(wizard.getByRole('heading', { name: 'Configure the Agent inline' })).toBeVisible();
  await expect(wizard.getByRole('radio', { name: /System default image/ })).toBeChecked();
  await wizard.getByLabel('Model', { exact: true }).fill('gpt-5');
  await wizard.getByRole('button', { name: 'Reasoning effort', exact: true }).click();
  await page
    .getByRole('menu', { name: 'Reasoning effort', exact: true })
    .getByRole('menuitemradio', { name: 'high', exact: true })
    .click();
  await wizard.getByRole('button', { name: 'Permission level', exact: true }).click();
  await page
    .getByRole('menu', { name: 'Permission level', exact: true })
    .getByRole('menuitemradio', { name: 'Guarded', exact: true })
    .click();
  await wizard.getByLabel('System prompt', { exact: true }).fill('Respond with concise updates.');
  await wizard.getByRole('button', { name: 'Continue', exact: true }).click();
  await wizard.getByRole('button', { name: 'Admission policy', exact: true }).click();
  await page
    .getByRole('menu', { name: 'Admission policy', exact: true })
    .getByRole('menuitemradio', { name: 'Private chats only', exact: true })
    .click();
  await wizard.getByLabel('Acknowledgement delay (seconds)', { exact: true }).fill('0');
  await wizard.getByRole('button', { name: 'Continue', exact: true }).click();
  const creation = page.waitForRequest(
    (request) => request.method() === 'POST' && new URL(request.url()).pathname === '/api/v1/bots',
  );
  await wizard.getByRole('button', { name: 'Create consumer', exact: true }).click();
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
  const detail = page.getByRole('region', { name: consumerName, exact: true });
  await expect(detail.getByRole('heading', { name: consumerName, exact: true })).toBeVisible();
  await expect(detail.getByText('Active', { exact: true })).toBeVisible();
  await expect(detail.getByRole('heading', { name: 'Event Gateway connection' })).toBeVisible();
  await consumers.getByRole('button', { name: 'Refresh consumers', exact: true }).click();
  const consumerRow = consumers.getByRole('button', { name: consumerName });
  await expect(consumerRow).toHaveAttribute('aria-current', 'page');
  for (const theme of ['light', 'dark']) {
    await page.evaluate(
      (value) => document.body.toggleAttribute('data-ds-dark-theme', value === 'dark'),
      theme,
    );
    for (const width of [1440, 390, 320]) {
      await page.setViewportSize({ width, height: 1000 });
      await expect
        .poll(() =>
          detail.evaluate((element) =>
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
  await detail.getByRole('button', { name: 'Run synthetic test', exact: true }).click();
  await expect(detail.getByText(/Accepted after \d+ ms/)).toBeVisible();
  await detail.getByRole('button', { name: 'Disable consumer', exact: true }).click();
  await expect(detail.getByText('Disabled', { exact: true })).toBeVisible();
  await detail.getByRole('button', { name: 'Enable consumer', exact: true }).click();
  await expect(detail.getByText('Active', { exact: true })).toBeVisible();
  await detail.getByRole('button', { name: 'Rotate keys', exact: true }).click();
  await detail.getByRole('button', { name: 'Confirm key rotation', exact: true }).click();
  await expect(detail.getByRole('button', { name: 'Delete consumer', exact: true })).toBeEnabled();
  await detail.getByRole('button', { name: 'Delete consumer', exact: true }).click();
  const deletion = page.getByRole('dialog', { name: `Delete ${consumerName}`, exact: true });
  await expect(deletion.getByRole('button', { name: 'Confirm deletion' })).toBeDisabled();
  await deletion
    .getByRole('checkbox', {
      name: 'I understand this Bot connection will be removed.',
      exact: true,
    })
    .check();
  await deletion.getByRole('button', { name: 'Confirm deletion', exact: true }).click();
  await expect(page.getByRole('status').filter({ hasText: /Consumer deleted\./ })).toBeVisible();
  await page.setViewportSize({ width: 1440, height: 1000 });
  await consumers.getByRole('button', { name: 'Refresh consumers', exact: true }).click();
  await expect(consumerRow).toHaveCount(0);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
