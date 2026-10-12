import { test, expect } from '@playwright/test';
import { LocalStartup, recorded } from './agent-startup-fixtures';

test('LOCAL Bot dashboard uses real CMA and CMAEG for creation, test and cleanup', async ({
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
  await page.route(/\/api\/v1\/agent-profiles(?:\?.*)?$/, (route) =>
    route.fulfill({
      json: {
        items: [
          {
            ...recorded<object>('profiles.json', 'agent-profile-read'),
            id: 'agp_fixture',
            name: 'Research profile',
            latest_revision: {
              ...recorded<object>('profiles.json', 'agent-profile-exact-revision'),
              profile_id: 'agp_fixture',
              revision: 1,
            },
          },
        ],
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
  await page.getByRole('button', { name: 'New Bot', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Continue' })).toBeDisabled();
  await page.getByLabel('Bot name', { exact: true }).fill('Browser acceptance Bot');
  await page.getByRole('button', { name: 'Telegram bot', exact: true }).click();
  await page.getByRole('menuitemradio', { name: 'Telegram research' }).click();
  await page.getByRole('button', { name: 'Continue' }).click();
  await page.getByRole('button', { name: /Select Research profile Version/ }).click();
  await page.getByRole('button', { name: 'Continue' }).click();
  await page.getByRole('button', { name: 'Admission policy' }).click();
  await page.getByRole('menuitemradio', { name: 'Private chats only', exact: true }).click();
  await page.getByLabel('Acknowledgement delay (seconds)', { exact: true }).fill('0');
  await page.getByRole('button', { name: 'Continue' }).click();
  await page.getByRole('button', { name: 'Create Bot', exact: true }).click();
  await expect(page.getByText('Active', { exact: true })).toBeVisible();
  await expect(page.locator('.bots__callback')).toContainText('/v1/callbacks/');
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
  await page.getByRole('button', { name: 'Disable Bot', exact: true }).click();
  await expect(page.getByText('Disabled', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Enable Bot', exact: true }).click();
  await expect(page.getByText('Active', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Rotate keys', exact: true }).click();
  await page.getByRole('button', { name: 'Confirm key rotation' }).click();
  await expect(page.getByRole('button', { name: 'Delete Bot', exact: true })).toBeEnabled();
  await page.getByRole('button', { name: 'Delete Bot', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Confirm deletion' })).toBeDisabled();
  await page.getByRole('checkbox').check();
  await page.getByRole('button', { name: 'Confirm deletion' }).click();
  await expect(page.getByText(/Bot deleted\./)).toBeVisible();
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
});
