import { expect, test, type Locator } from '@playwright/test';
import type { Bot } from '../src/api-client';
import { LocalStartup, recorded } from './agent-startup-fixtures';
import {
  channelCapabilities,
  cmaSession,
  gatewaySession,
  teamChannel,
} from '../build/fixtures/eventGateway';

const owner = '00000000-0000-4000-8000-000000000383';
const createdAt = '2026-11-07T00:30:12.123456789Z';

async function conciseRow(row: Locator, label: string, date: string) {
  await expect(row).toHaveAccessibleName(`${label} , Created ${date}`);
  await expect(row.getByRole('button')).toHaveAccessibleName(`${label} , Created ${date}`);
  await expect(row.locator('time')).toHaveText(date);
  await expect(row.locator('time')).toHaveAttribute('datetime', createdAt);
  await expect(row.locator('.sr-only')).toHaveText(`, Created ${date}`);
  await expect(row.locator('.tree-row__age')).toHaveAttribute('title', `Created ${date}`);
  const nameSources = await row.evaluate((element) =>
    [element, ...element.querySelectorAll('*')]
      .map((node) => [
        node.getAttribute('aria-label'),
        node.getAttribute('title'),
        node.textContent,
      ])
      .flat()
      .join(' '),
  );
  expect(nameSources).not.toContain(owner);
  expect(nameSources).not.toContain(createdAt);
  expect(nameSources).not.toContain('By ');
}

for (const settings of [
  { locale: 'en-US', timezoneId: 'Asia/Tokyo', date: 'Nov 7' },
  { locale: 'fr-FR', timezoneId: 'America/Los_Angeles', date: '6 nov.' },
]) {
  test.describe(`${settings.locale} ${settings.timezoneId}`, () => {
    test.use({ locale: settings.locale, timezoneId: settings.timezoneId });

    test('LOCAL channel and consumer rows keep concise names, focus and selection', async ({
      page,
      baseURL,
    }, info) => {
      const boundary = new LocalStartup();
      await boundary.route(page, baseURL!);
      const reads: string[] = [];
      await page.route(`${baseURL}/auth/session`, (route) => route.fulfill({ json: cmaSession }));
      await page.route(`${baseURL}/api/v1/telemetry/browser`, (route) => {
        expect(route.request().method()).toBe('POST');
        expect(route.request().postDataJSON()).toEqual({ kind: 'uncaught_error' });
        return route.fulfill({ status: 202, json: {} });
      });
      const channels = ['first', 'second', 'invalid', 'missing'].map((id, index) => ({
        ...teamChannel,
        record_id: `record-${id}`,
        channel_id: `channel-${id}`,
        definition: { ...teamChannel.definition, name: index < 2 ? 'Updates' : `${id} date` },
        created_at: index === 2 ? 'not-a-date' : index === 3 ? undefined : createdAt,
        owner: index === 0 ? owner : { subject: owner, display_name: '' },
      }));
      await page.route(`${baseURL}/api/event-gateway/**`, (route) => {
        const { pathname } = new URL(route.request().url());
        if (route.request().method() === 'GET') {
          reads.push(pathname);
          if (pathname.endsWith('/auth/session')) return route.fulfill({ json: gatewaySession });
          if (pathname.endsWith('/v1/channels'))
            return route.fulfill({ json: { items: channels, next_cursor: null } });
          if (pathname.endsWith('/v1/channel-capabilities'))
            return route.fulfill({ json: channelCapabilities });
          if (pathname.endsWith('/v1/connections')) return route.fulfill({ json: { items: [] } });
          if (/\/channels\/channel-(first|second)\/activity\/stream$/.test(pathname))
            return route.fulfill({ contentType: 'text/event-stream', body: ': no activity\n\n' });
        }
        boundary.unexpected.push(`${route.request().method()} ${pathname}`);
        return route.abort();
      });
      const consumers = ['first', 'second'].map((id) => ({
        ...recorded<Bot>('bots.json', 'bots-get-success'),
        bot_id: `consumer-${id}`,
        name: 'Research assistant',
        owner,
        created_at: createdAt,
      }));
      await page.route(`${baseURL}/api/v1/bots**`, (route) => {
        const { pathname } = new URL(route.request().url());
        if (route.request().method() === 'GET') {
          reads.push(pathname);
          if (pathname === '/api/v1/bots')
            return route.fulfill({ json: { items: consumers, next_after: null } });
          const consumer = consumers.find((item) => pathname === `/api/v1/bots/${item.bot_id}`);
          if (consumer) return route.fulfill({ json: consumer });
        }
        boundary.unexpected.push(`${route.request().method()} ${pathname}`);
        return route.abort();
      });
      await page.setViewportSize({ width: 1440, height: 1000 });
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.goto('/event-gateway');
      const tree = page.getByRole('tree', { name: 'Channels', exact: true });
      const rows = tree.getByRole('treeitem');
      await expect(rows).toHaveCount(4);
      for (let index = 0; index < 2; index++)
        await conciseRow(rows.nth(index), 'Updates', settings.date);
      for (let index = 2; index < 4; index++) {
        await expect(rows.nth(index)).toHaveAccessibleName(
          `${index === 2 ? 'invalid' : 'missing'} date , Created Not reported`,
        );
        await expect(rows.nth(index).locator('time')).toHaveCount(0);
      }
      await rows.nth(0).focus();
      await page.keyboard.press('ArrowDown');
      await expect(rows.nth(1)).toBeFocused();
      await page.keyboard.press('Enter');
      await expect(rows.nth(1)).toHaveAttribute('aria-selected', 'true');
      await expect(rows.nth(1).getByRole('button')).toHaveAttribute('aria-current', 'page');
      await expect(page.getByRole('heading', { name: 'Updates', exact: true })).toBeVisible();
      await expect
        .poll(() => reads)
        .toContain('/api/event-gateway/v1/channels/channel-second/activity/stream');
      await page.keyboard.press('ArrowUp');
      await expect(rows.nth(0)).toBeFocused();
      await page.keyboard.press('Enter');
      await expect(rows.nth(0)).toHaveAttribute('aria-selected', 'true');
      await expect(rows.nth(1)).toHaveAttribute('aria-selected', 'false');
      await expect
        .poll(() => reads)
        .toContain('/api/event-gateway/v1/channels/channel-first/activity/stream');
      await info.attach('channel-accessibility-tree', {
        body: await tree.ariaSnapshot(),
        contentType: 'text/plain',
      });
      await page
        .getByRole('group', { name: 'Navigation view' })
        .getByRole('button', { name: 'Consumers', exact: true })
        .click();
      const consumerRows = page
        .getByRole('tree', { name: 'Consumers', exact: true })
        .getByRole('treeitem');
      await expect(consumerRows).toHaveCount(2);
      for (let index = 0; index < 2; index++)
        await conciseRow(consumerRows.nth(index), 'Research assistant', settings.date);
      await consumerRows.nth(0).focus();
      await page.keyboard.press('ArrowDown');
      await expect(consumerRows.nth(1)).toBeFocused();
      await page.keyboard.press('Enter');
      await expect(page).toHaveURL(/\/bots\/consumer-second$/);
      await expect(consumerRows.nth(1)).toHaveAttribute('aria-selected', 'true');
      await expect(
        page.getByRole('heading', { name: 'Research assistant', exact: true }),
      ).toBeVisible();
      await expect(page.getByText('Active', { exact: true })).toBeVisible();
      await page.keyboard.press('ArrowUp');
      await expect(consumerRows.nth(0)).toBeFocused();
      await page.keyboard.press('Enter');
      await expect(page).toHaveURL(/\/bots\/consumer-first$/);
      await expect(consumerRows.nth(0)).toHaveAttribute('aria-selected', 'true');
      await expect(consumerRows.nth(1)).toHaveAttribute('aria-selected', 'false');
      expect(reads.filter((path) => path.startsWith('/api/v1/bots'))).toEqual([
        '/api/v1/bots',
        '/api/v1/bots/consumer-second',
        '/api/v1/bots/consumer-first',
      ]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.writes).toEqual([]);
      // The unmodified channel map also emits this notification in WebKit.
      const mapWarnings = boundary.errors.filter(
        (error) =>
          info.project.name === 'webkit' &&
          error === 'ResizeObserver loop completed with undelivered notifications.',
      );
      expect(boundary.errors).toEqual(mapWarnings);
      if (mapWarnings.length > 0) {
        console.warn('Existing WebKit channel-map notification:', mapWarnings);
        await info.attach('existing-map-notifications', {
          body: JSON.stringify(mapWarnings),
          contentType: 'application/json',
        });
      }
      await info.attach('consumer-accessibility-tree', {
        body: await page.getByRole('tree', { name: 'Consumers', exact: true }).ariaSnapshot(),
        contentType: 'text/plain',
      });
    });
  });
}
