import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

test('LOCAL channel map preserves node drafts and renders every producer and consumer in both themes', async ({
  page,
  baseURL,
}, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  const channel = {
    channel_id: 'channel-local',
    record_id: 'record-local',
    version: 3,
    provider: { slug: 'cma_codex', level: 'l2' },
    definition: {
      name: 'Team workspace',
      schema_version: 1,
      provider: { slug: 'cma_codex', binding_id: 'binding-local' },
      sources: ['telegram', 'whatsapp', 'lark', 'discord'].map((platform) => ({
        type: 'nyxid_relay',
        platform,
        issuer: 'https://nyx.example',
        key_id: 'public-signing-key',
        route_ids: [`${platform}-team-route`],
        admission: { type: 'all' },
      })),
      partition: { version: 1, key: 'conversation_and_sender' },
      reply: { auto_final: true, ack_after_ms: 1500, sinks: [{ type: 'source' }] },
      routing: {
        version: 1,
        default_consumer: { name: 'assistant', label: 'CMA Agent' },
        default_targets: ['default'],
        consumers: [
          {
            id: 'reviewer',
            name: 'reviewer',
            label: 'Nyxbot reviewer',
            provider: { slug: 'nyxbot', binding_id: 'binding-reviewer' },
          },
        ],
        rules: [],
      },
    },
    endpoints: {
      custom_event_url: 'https://gateway.example/events',
      nyxid_callback_url: 'https://gateway.example/callback',
    },
  };
  let writes = 0;
  await page.route('**/api/event-gateway/**', async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (route.request().method() !== 'GET') writes++;
    if (path.endsWith('/auth/session'))
      return route.fulfill({
        json: {
          authenticated: true,
          subject: 'local-owner',
          permissions: ['cma-eg:admin'],
          expires_at: 9999999999,
        },
      });
    if (path.endsWith('/v1/channels'))
      return route.fulfill({ json: { items: [channel], next_cursor: null } });
    return route.fulfill({ status: 404, json: { detail: 'Unexpected local request' } });
  });
  await page.setViewportSize({ width: 1600, height: 1000 });
  await page.goto('/event-gateway');
  await page
    .getByRole('table', { name: 'Your channels' })
    .getByRole('button', { name: 'Team workspace' })
    .click();
  const map = page.getByRole('region', { name: 'Delivery map' });
  await expect(map.getByRole('button')).toHaveCount(8);
  const editor = page.locator('.channel-node-editor');
  const retained = await editor.elementHandle();
  await page.getByLabel('Channel name', { exact: true }).fill('Keep this channel draft');
  await map.getByRole('button', { name: 'Producer: Telegram (NyxID relay)' }).click();
  await expect(page.getByLabel('NyxID issuer', { exact: true })).toBeVisible();
  await expect(page.getByLabel('Channel name', { exact: true })).toHaveCount(0);
  await page.getByLabel('Bot username', { exact: true }).fill('retained_bot');
  await map.getByRole('button', { name: 'Consumer: Nyxbot reviewer (nyxbot)' }).click();
  await expect(page.getByLabel('NyxID issuer', { exact: true })).toHaveCount(0);
  await map.getByRole('button', { name: /Channel: Team workspace/ }).click();
  await expect(page.getByLabel('Channel name', { exact: true })).toHaveValue(
    'Keep this channel draft',
  );
  await map.getByRole('button', { name: 'Producer: Telegram (NyxID relay)' }).click();
  await expect(page.getByLabel('Bot username', { exact: true })).toHaveValue('retained_bot');
  expect(await editor.evaluate((element, previous) => element === previous, retained)).toBe(true);
  for (const theme of ['light', 'dark']) {
    await page.evaluate(
      (value) => document.body.toggleAttribute('data-ds-dark-theme', value === 'dark'),
      theme,
    );
    for (const width of [1600, 390]) {
      await page.setViewportSize({ width, height: 1000 });
      await expect
        .poll(() =>
          map.evaluate((element) => {
            const rect = element.getBoundingClientRect();
            return (
              rect.left >= 0 &&
              rect.right <= innerWidth + 1 &&
              element.scrollWidth <= element.clientWidth + 1
            );
          }),
        )
        .toBe(true);
      await map.scrollIntoViewIfNeeded();
      await page.screenshot({
        path: info.outputPath(`channel-map-${theme}-${width}.png`),
        fullPage: true,
        animations: 'disabled',
      });
      const inspector = page.locator('.channel-inspector');
      await inspector.scrollIntoViewIfNeeded();
      await expect
        .poll(() =>
          inspector.evaluate(
            (element) =>
              element.scrollWidth <= element.clientWidth + 1 &&
              [...element.querySelectorAll('input,button,textarea')]
                .filter((item) => item.getClientRects().length > 0)
                .every((item) => {
                  const rect = item.getBoundingClientRect();
                  return rect.left >= 0 && rect.right <= innerWidth + 1;
                }),
          ),
        )
        .toBe(true);
      await page.screenshot({
        path: info.outputPath(`channel-inspector-${theme}-${width}.png`),
        fullPage: true,
        animations: 'disabled',
      });
    }
  }
  await page.emulateMedia({ reducedMotion: 'reduce' });
  expect(
    await map
      .getByRole('button')
      .first()
      .evaluate((element) => getComputedStyle(element).transitionDuration),
  ).toBe('0s');
  expect(writes).toBe(0);
  expect(boundary.errors).toEqual([]);
});
