import { writeFileSync } from 'node:fs';
import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

test('LOCAL channel map preserves node drafts and renders every producer and consumer in both themes', async ({
  page,
  baseURL,
}, info) => {
  const themeEvidence: Record<string, unknown>[] = [];
  const capture = async (file: string, theme: 'light' | 'dark') => {
    const expected = {
      darkAttribute: theme === 'dark',
      colorScheme: theme,
      pageBackground: theme === 'dark' ? 'rgb(9, 9, 11)' : 'rgb(255, 255, 255)',
      canvasBackground: theme === 'dark' ? 'rgb(24, 24, 27)' : 'rgb(250, 250, 250)',
    };
    const rendered = () =>
      page.evaluate(() => ({
        darkAttribute: document.body.hasAttribute('data-ds-dark-theme'),
        colorScheme: getComputedStyle(document.documentElement).colorScheme,
        pageBackground: getComputedStyle(document.querySelector('.service-page')!).backgroundColor,
        canvasBackground: getComputedStyle(document.querySelector('.delivery-flow')!)
          .backgroundColor,
      }));
    await expect.poll(rendered).toEqual(expected);
    themeEvidence.push({ file, theme, ...(await rendered()) });
    await page.screenshot({ path: info.outputPath(file), fullPage: true, animations: 'disabled' });
  };
  // The product intentionally defaults to dark. A light screenshot must explicitly
  // select light before bootstrap, rather than merely use a light filename.
  await page.addInitScript(() => localStorage.setItem('cma:appearance', 'light'));
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  await page.route('**/auth/session', (route) =>
    route.fulfill({
      json: {
        authenticated: true,
        subject: 'local-owner',
        display_name: 'Local owner',
        capabilities: [
          'use_own_sandboxes',
          'create_sandbox',
          'delete_own_sandbox',
          'manage_own_bots',
        ],
      },
    }),
  );
  await page.route('**/api/v1/bots?**', (route) =>
    route.fulfill({ json: { items: [], next_after: null } }),
  );
  const channel = {
    channel_id: 'channel-local',
    record_id: 'record-local',
    version: 3,
    created_at: '2026-10-27T01:00:00Z',
    owner: { subject: 'local-owner', display_name: 'Local owner' },
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
    if (path.endsWith('/v1/channel-capabilities'))
      return route.fulfill({
        json: {
          relay_authoring: {
            dynamic_sources_writes_enabled: true,
            relay_raw_verification_writes_enabled: true,
            outbound_messages_writes_enabled: true,
            native_media_writes_enabled: true,
            multi_consumer_writes_enabled: true,
            native_bot_mentions_writes_enabled: true,
            thread_contract_writes_enabled: true,
            platforms: ['telegram', 'whatsapp', 'lark', 'discord'].map((id) => ({
              id,
              label: id,
              verification_modes: [
                { id: 'raw_event', writes_enabled: true },
                { id: 'nyxid_normalized', writes_enabled: true },
              ],
            })),
          },
        },
      });
    if (path.endsWith('/v1/connections')) return route.fulfill({ json: { items: [] } });
    if (path.endsWith('/activity/stream'))
      return route.fulfill({
        contentType: 'text/event-stream',
        body: `id: activity-fixture\nevent: activity\ndata: ${JSON.stringify({
          cursor: 'activity-fixture',
          retention_days: 7,
          limit: 50,
          has_more: false,
          items: [
            {
              event_id: 'event-fixture',
              source: { type: 'nyxid_relay', platform: 'telegram' },
              actor: { id: 'producer-fixture', display_name: 'Project updates', kind: 'service' },
              conversation: { id: 'team-chat', kind: 'group' },
              occurred_at: '2026-10-27T01:30:00Z',
              received_at: '2026-10-27T01:30:01Z',
              consumers: [
                {
                  consumer_id: 'reviewer',
                  display_name: 'Nyxbot reviewer',
                  agent_id: 'agent-reviewer',
                  provider: 'nyxbot',
                  conversation_id: 'review-conversation',
                  state: 'completed',
                  dispatched_at: '2026-10-27T01:30:02Z',
                  accepted_at: '2026-10-27T01:30:03Z',
                  produced_at: '2026-10-27T01:30:04Z',
                  completed_at: '2026-10-27T01:30:05Z',
                  response_id: 'response-fixture',
                  deliveries: [
                    {
                      step: 'answer',
                      destination: 'origin',
                      outcome: 'confirmed',
                      observed_at: '2026-10-27T01:30:05Z',
                      confirmed_at: '2026-10-27T01:30:05Z',
                      platform_message_id: 'message-fixture',
                      recipients: [{ kind: 'telegram_chat', id: 'team-chat', thread_id: null }],
                    },
                  ],
                },
              ],
            },
          ],
        })}\n\n`,
      });
    if (path.endsWith('/v1/channels'))
      return route.fulfill({ json: { items: [channel], next_cursor: null } });
    return route.fulfill({ status: 404, json: { detail: 'Unexpected local request' } });
  });
  await page.setViewportSize({ width: 1600, height: 1000 });
  await page.goto('/event-gateway');
  await expect(page.getByRole('region', { name: 'Channel workspace' })).toBeVisible();
  await expect(
    page.getByLabel('Illustrative event journey').locator('.channel-map__node'),
  ).toHaveCount(4);
  await expect(
    page.getByRole('group', { name: 'Navigation view' }).getByRole('button', { name: 'Consumers' }),
  ).toBeVisible();
  await capture('channel-welcome-light-1600.png', 'light');
  await page
    .getByRole('region', { name: 'Channels' })
    .getByRole('treeitem', { name: /^Team workspace/ })
    .click();
  const map = page.getByRole('region', { name: 'Delivery map' });
  await expect(map.getByRole('button', { name: /^(Producer|Channel|Consumer):/ })).toHaveCount(7);
  await map.getByRole('button', { name: /Channel: Team workspace/ }).click();
  const editor = page.locator('.channel-node-editor');
  const retained = await editor.elementHandle();
  await page.getByLabel('Channel name', { exact: true }).fill('Keep this channel draft');
  await page.getByRole('button', { name: 'Close', exact: true }).click();
  await map.getByRole('button', { name: 'Producer: Telegram (NyxID relay)' }).click();
  await expect(page.getByLabel('NyxID issuer', { exact: true })).toBeVisible();
  await expect(page.getByLabel('Channel name', { exact: true })).toHaveCount(0);
  await page.getByLabel('Bot username', { exact: true }).fill('retained_bot');
  await page.getByRole('button', { name: 'Close', exact: true }).click();
  await map.getByRole('button', { name: 'Consumer: Nyxbot reviewer (nyxbot)' }).click();
  await expect(page.getByLabel('NyxID issuer', { exact: true })).toHaveCount(0);
  await page.getByRole('button', { name: 'Close', exact: true }).click();
  await map.getByRole('button', { name: /Channel: Team workspace/ }).click();
  await expect(page.getByLabel('Channel name', { exact: true })).toHaveValue(
    'Keep this channel draft',
  );
  await page.getByRole('button', { name: 'Close', exact: true }).click();
  await map.getByRole('button', { name: 'Producer: Telegram (NyxID relay)' }).click();
  await expect(page.getByLabel('Bot username', { exact: true })).toHaveValue('retained_bot');
  expect(await editor.evaluate((element, previous) => element === previous, retained)).toBe(true);
  await page.getByRole('button', { name: 'Close', exact: true }).click();
  await map.getByRole('button', { name: /^Toolsets: Agent toolsets/ }).click();
  await expect(page.getByText('readEventContext', { exact: true })).toBeVisible();
  await expect(
    page
      .getByRole('navigation', { name: 'Configuration sections' })
      .getByRole('button', { name: 'Optional tools' }),
  ).toBeVisible();
  await page.getByRole('button', { name: 'Close', exact: true }).click();
  await map.getByRole('button', { name: 'Producer: Telegram (NyxID relay)' }).click();
  await expect(page.getByLabel('Bot username', { exact: true })).toHaveValue('retained_bot');
  await page.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(map.getByText('Browse delivery nodes')).toHaveCount(0);
  await expect(map.locator('.react-flow__edge-path[marker-end]')).toHaveCount(8);
  for (const theme of ['light', 'dark'] as const) {
    await page.evaluate((value) => {
      localStorage.setItem('cma:appearance', value);
      document.body.toggleAttribute('data-ds-dark-theme', value === 'dark');
      document.documentElement.style.colorScheme = value;
    }, theme);
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
      await expect
        .poll(() =>
          map.locator('.delivery-flow').evaluate((element) => {
            const bounds = element.getBoundingClientRect();
            const cards = [...element.querySelectorAll('.channel-map__node')].map((card) =>
              card.getBoundingClientRect(),
            );
            const left = Math.min(...cards.map((card) => card.left));
            const right = Math.max(...cards.map((card) => card.right));
            return Math.abs((left + right) / 2 - (bounds.left + bounds.right) / 2);
          }),
        )
        .toBeLessThan(3);
      await capture(`channel-map-${theme}-${width}.png`, theme);
      await map.getByRole('button', { name: 'Producer: Telegram (NyxID relay)' }).click();
      const inspector = page.locator('.channel-inspector');
      const bounds = await page.getByRole('dialog').boundingBox();
      expect(bounds!.width).toBeLessThanOrEqual(width * 0.6 + 1);
      expect(bounds!.height).toBeLessThanOrEqual(601);
      expect(bounds!.x).toBeGreaterThanOrEqual(width * 0.2 - 1);
      expect(bounds!.y).toBeGreaterThanOrEqual(199);
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
      await capture(`channel-inspector-${theme}-${width}.png`, theme);
      await page
        .getByRole('navigation', { name: 'Configuration sections' })
        .getByRole('button', { name: 'NyxID connection', exact: true })
        .click();
      await expect(page.getByLabel('NyxID issuer', { exact: true })).toBeVisible();
      await expect
        .poll(() =>
          page
            .getByRole('group', { name: 'NyxID connection', exact: true })
            .evaluate((element) =>
              Math.abs(
                element.getBoundingClientRect().top -
                  element.closest('.channel-inspector')!.getBoundingClientRect().top,
              ),
            ),
        )
        .toBeLessThan(36);
      await capture(`channel-fields-${theme}-${width}.png`, theme);
      await page.getByRole('button', { name: 'Close', exact: true }).click();
    }
  }
  await page.setViewportSize({ width: 1600, height: 1000 });
  await map.getByRole('button', { name: 'Activity', exact: true }).click();
  await expect(page.getByText('Delivery confirmed')).toBeVisible();
  await expect(page.getByText('telegram_chat: team-chat', { exact: false })).toBeVisible();
  await expect(page.getByText('agent-reviewer', { exact: true })).toHaveCount(1);
  await expect
    .poll(() =>
      page.getByRole('region', { name: 'Activity', exact: true }).evaluate((element) => {
        const container = element.closest('.channel-inspector')!;
        return Math.abs(
          element.getBoundingClientRect().top - container.getBoundingClientRect().top,
        );
      }),
    )
    .toBeLessThan(32);
  await capture('channel-activity-dark-1600.png', 'dark');
  writeFileSync(info.outputPath('theme-evidence.json'), JSON.stringify(themeEvidence, null, 2));
  await page.getByRole('button', { name: 'Close', exact: true }).click();
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
