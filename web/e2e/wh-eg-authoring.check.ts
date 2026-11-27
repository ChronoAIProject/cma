import { expect, test, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import {
  channelActivityFrame,
  channelCapabilities,
  cmaSession,
  gatewaySession,
  teamChannel,
} from '../build/fixtures/eventGateway';

const longName =
  'Customer-support-channel-with-a-long-distinct-name-for-mobile-identity-and-triage';
async function fixture(page: Page, baseURL: string) {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL);
  await page.route('**/auth/session', (route) => route.fulfill({ json: cmaSession }));
  await page.route('**/api/v1/bots?**', (route) =>
    route.fulfill({ json: { items: [], next_after: null } }),
  );
  const writes: { body: string; key: string }[] = [];
  await page.route('**/api/event-gateway/**', async (route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    if (request.method() !== 'GET') {
      writes.push({ body: request.postData()!, key: request.headers()['idempotency-key']! });
      return route.fulfill({
        status: 400,
        json: { detail: '/sources/0/bot_id: Check the selected bot identity.' },
      });
    }
    if (path.endsWith('/auth/session')) return route.fulfill({ json: gatewaySession });
    if (path.endsWith('/v1/channel-capabilities'))
      return route.fulfill({ json: channelCapabilities });
    if (path.endsWith('/v1/connections')) return route.fulfill({ json: { items: [] } });
    if (path.endsWith('/activity/stream'))
      return route.fulfill({ contentType: 'text/event-stream', body: channelActivityFrame });
    if (path.endsWith('/v1/channels'))
      return route.fulfill({
        json: {
          items: [{ ...teamChannel, definition: { ...teamChannel.definition, name: longName } }],
          next_cursor: null,
        },
      });
    return route.fulfill({ status: 404, json: { detail: 'Unexpected local request' } });
  });
  return writes;
}

for (const theme of ['light', 'dark'] as const) {
  test(`LOCAL mobile channel identity and producer navigation remain readable in ${theme}`, async ({
    page,
    baseURL,
  }, info) => {
    await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
    const writes = await fixture(page, baseURL!);
    await page.setViewportSize({ width: 390, height: 844 });
    await page.goto('/event-gateway');
    const channelItem = page.getByRole('treeitem', { name: new RegExp(`^${longName}`) });
    // Mobile navigation lives in the shared drawer.
    if (!(await channelItem.isVisible()))
      await page.getByRole('button', { name: 'Show navigation' }).click();
    await channelItem.click();
    const heading = page.getByRole('heading', { name: longName, exact: true });
    await expect(heading).toBeVisible();
    const crumb = page.locator(
      '.gateway-detail:not([hidden]) .page-header__crumbs [aria-current="page"]',
    );
    await page.screenshot({
      path: info.outputPath(`before-geometry-${theme}.png`),
      fullPage: true,
    });
    for (const width of [390, 320, 720, 1440]) {
      await page.setViewportSize({ width, height: 844 });
      for (const text of [heading, crumb]) {
        expect(
          await text.evaluate((element) => {
            const range = document.createRange();
            range.selectNodeContents(element);
            const bounds = element.getBoundingClientRect();
            return [...range.getClientRects()].every(
              (rect) => rect.left >= bounds.left - 1 && rect.right <= bounds.right + 1,
            );
          }),
        ).toBe(true);
      }
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      );
      await page.screenshot({
        path: info.outputPath(`identity-${theme}-${width}.png`),
        fullPage: true,
      });
    }
    await page.setViewportSize({ width: 390, height: 844 });
    await page
      .getByRole('region', { name: 'Delivery map' })
      .getByRole('button', { name: 'Producer: Telegram (NyxID relay)' })
      .click();
    const navigation = page.getByRole('navigation', { name: 'Configuration sections' });
    await expect(navigation).toBeVisible();
    for (const button of await navigation.getByRole('button').all()) {
      await button.focus();
      await expect(button).toBeInViewport();
      await button.click();
      await expect(button).toHaveAttribute('aria-current', 'location');
      expect((await button.boundingBox())!.height).toBeGreaterThanOrEqual(44);
    }
    await page.screenshot({ path: info.outputPath(`navigation-${theme}-390.png`) });
    expect(writes).toEqual([]);
  });
}

test('LOCAL authoring retains drafts, warns on reload, validates before sending and reviews the exact rejected field', async ({
  page,
  baseURL,
}, info) => {
  const writes = await fixture(page, baseURL!);
  await page.setViewportSize({ width: 1440, height: 960 });
  await page.goto('/event-gateway');
  await page.getByRole('button', { name: 'New channel', exact: true }).first().click();
  const choose = async (name: string, value: string) => {
    await page.getByRole('button', { name, exact: true }).click();
    await page.getByRole('menuitemradio', { name: value, exact: true }).click();
  };
  await page.getByLabel('Channel name', { exact: true }).fill('Retained authoring draft');
  await page.getByLabel('Provider agent key', { exact: true }).fill('synthetic-provider');
  await page.getByLabel('NyxID issuer', { exact: true }).fill('https://relay.example.test');
  await page.getByLabel('Signing key ID', { exact: true }).fill('retained-signing-key');
  await page.getByLabel('NyxID channel key', { exact: true }).fill('synthetic-relay-draft');
  await expect(page.getByLabel('Enable verified thread following')).toBeDisabled();
  await choose('Producer 1 connection type', 'Direct Telegram bot');
  const bot = page.getByLabel('Bot ID', { exact: true });
  await bot.fill('not-numeric');
  await page.getByRole('button', { name: 'Create channel', exact: true }).click();
  await expect(bot).toBeFocused();
  await expect(bot).toHaveAttribute('aria-invalid', 'true');
  expect(writes).toEqual([]);
  await bot.fill('12345');
  const ack = page.getByLabel('Acknowledgement delay (ms)');
  await ack.fill('999999999999999');
  await page.getByRole('button', { name: 'Create channel', exact: true }).click();
  await expect(ack).toBeFocused();
  expect(writes).toEqual([]);
  await ack.fill('60000');
  await choose('Producer 1 connection type', 'NyxID messaging connection');
  await expect(page.getByLabel('NyxID issuer', { exact: true })).toHaveValue(
    'https://relay.example.test',
  );
  await expect(page.getByLabel('Signing key ID', { exact: true })).toHaveValue(
    'retained-signing-key',
  );
  await expect(page.getByLabel('NyxID channel key', { exact: true })).toHaveValue(
    'synthetic-relay-draft',
  );
  const dialog = page.waitForEvent('dialog');
  const reloading = page.evaluate(() => window.location.reload());
  const warning = await dialog;
  expect(warning.type()).toBe('beforeunload');
  await warning.dismiss();
  await reloading;
  await expect(page.getByLabel('Channel name', { exact: true })).toHaveValue(
    'Retained authoring draft',
  );
  expect(
    await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage })),
  ).not.toContain('synthetic-relay-draft');
  await choose('Producer 1 connection type', 'Direct Telegram bot');
  await expect(bot).toHaveValue('12345');
  const details = page.locator('.channel-node-editor details');
  const before = await details.evaluateAll((elements) =>
    elements.map((element) => (element as HTMLDetailsElement).open),
  );
  await page.getByRole('button', { name: 'Create channel', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('Bot ID: Check the selected bot identity.');
  await page.getByRole('button', { name: 'Review channel fields' }).click();
  await expect(bot).toBeFocused();
  expect(
    await details.evaluateAll((elements) =>
      elements.map((element) => (element as HTMLDetailsElement).open),
    ),
  ).toEqual(before);
  expect(writes).toHaveLength(1);
  expect(JSON.parse(writes[0]!.body).credentials).not.toHaveProperty('channel_key');
  await page.screenshot({ path: info.outputPath('exact-field-review.png'), fullPage: true });
  await choose('Producer 1 connection type', 'NyxID messaging connection');
  await expect(page.getByRole('alert')).toHaveCount(0);
  expect(writes).toHaveLength(1);
});

test('LOCAL browser refresh restores the owner’s New channel draft without retaining credentials', async ({
  page,
  baseURL,
}) => {
  const writes = await fixture(page, baseURL!);
  await page.goto('/event-gateway');
  await page.getByRole('button', { name: 'New channel', exact: true }).first().click();
  const name = page.getByLabel('Channel name', { exact: true });
  const issuer = page.getByLabel('NyxID issuer', { exact: true });
  const routeId = page.getByLabel('Route IDs 1', { exact: true });
  const key = page.getByLabel('Provider agent key', { exact: true });
  await name.fill('Retain this channel');
  await issuer.fill('https://relay.example.test');
  await routeId.fill('draft-route');
  await key.fill('synthetic-provider-reload-value');

  // Accept any unload warning. Reload must restore the draft either way.
  page.once('dialog', (dialog) => void dialog.accept());
  await page.reload();
  await expect(page.getByRole('heading', { name: 'New channel', exact: true })).toBeVisible();
  await expect(name).toHaveValue('Retain this channel');
  await expect(issuer).toHaveValue('https://relay.example.test');
  await expect(routeId).toHaveValue('draft-route');
  await expect(key).toHaveValue('');
  await expect(page.getByText(/Draft restored\. Re-enter/)).toBeVisible();
  expect(
    await page.evaluate(() => JSON.stringify({ ...localStorage, ...sessionStorage })),
  ).not.toContain('synthetic-provider-reload-value');
  expect(writes).toEqual([]);

  const otherTab = await page.context().newPage();
  try {
    const newTabWrites = await fixture(otherTab, baseURL!);
    await otherTab.goto('/event-gateway');
    await otherTab.getByRole('button', { name: 'New channel', exact: true }).first().click();
    await expect(otherTab.getByLabel('Channel name', { exact: true })).toHaveValue('');
    expect(newTabWrites).toEqual([]);
  } finally {
    await otherTab.close();
  }
});
