import { expect, test, type Locator, type Page } from '@playwright/test';
import type { Bot } from '../src/api-client';
import { LocalStartup, recorded } from './agent-startup-fixtures';
import {
  channelCapabilities,
  cmaSession,
  gatewaySession,
  teamChannel,
} from '../build/fixtures/eventGateway';
import { activityPage } from './redesign-channel.fixture';

const owner = '00000000-0000-4000-8000-000000000383';
const createdAt = '2026-11-07T00:30:12.123456789Z';

async function sectionDialog(page: Page, baseURL: string, theme: 'light' | 'dark') {
  const boundary = new LocalStartup();
  const name = 'Team updates with a long channel title and retained delivery information';
  await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
  await boundary.route(page, baseURL);
  await page.route(`${baseURL}/auth/session`, (route) => route.fulfill({ json: cmaSession }));
  await page.route(`${baseURL}/api/event-gateway/**`, (route) => {
    const request = route.request();
    const { pathname } = new URL(request.url());
    if (request.method() === 'GET') {
      if (pathname.endsWith('/auth/session')) return route.fulfill({ json: gatewaySession });
      if (pathname.endsWith('/v1/channels'))
        return route.fulfill({
          json: {
            items: [
              {
                ...teamChannel,
                definition: {
                  ...teamChannel.definition,
                  name,
                  routing: {
                    ...teamChannel.definition.routing,
                    default_consumer: { name: 'assistant', label: name },
                  },
                },
              },
            ],
            next_cursor: null,
          },
        });
      if (pathname.endsWith('/v1/channel-capabilities'))
        return route.fulfill({ json: channelCapabilities });
      if (pathname.endsWith('/v1/connections')) return route.fulfill({ json: { items: [] } });
      if (pathname.endsWith('/activity/stream'))
        return route.fulfill({
          contentType: 'text/event-stream',
          body: `event: activity\ndata: ${JSON.stringify(activityPage())}\n\n`,
        });
    }
    boundary.unexpected.push(`${request.method()} ${pathname}`);
    return route.abort();
  });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto('/event-gateway');
  await page.getByRole('treeitem', { name: new RegExp(`^${name}`) }).click();
  const opener = page
    .getByRole('region', { name: 'Delivery map' })
    .getByRole('button', { name: new RegExp(`^Channel: ${name}`) });
  await opener.click();
  const dialog = page.getByRole('dialog', { name: 'Channel settings', exact: true });
  await expect(dialog).toBeVisible();
  const longTitleOpener = page
    .getByRole('region', { name: 'Delivery map' })
    .getByRole('button', { name: `Consumer: ${name} (cma_codex)`, exact: true });
  return { boundary, dialog, opener, longTitleOpener, name };
}

for (const theme of ['light', 'dark'] as const) {
  for (const size of [
    { width: 1440, height: 900, label: 'desktop', motion: 'no-preference' },
    { width: 720, height: 900, label: 'tablet', motion: 'reduce' },
    { width: 390, height: 844, label: 'phone390', motion: 'no-preference' },
    { width: 360, height: 800, label: 'phone360', motion: 'reduce' },
    { width: 320, height: 720, label: 'phone320', motion: 'reduce' },
    { width: 720, height: 450, label: '200-percent-reflow', motion: 'reduce' },
  ] as const) {
    test(`LOCAL sections Modal ${theme} ${size.label} keeps readable content and keyboard access`, async ({
      page,
      baseURL,
    }, info) => {
      await page.emulateMedia({ reducedMotion: size.motion });
      const { boundary, dialog, opener, longTitleOpener, name } = await sectionDialog(
        page,
        baseURL!,
        theme,
      );
      expect(await page.locator('body').getAttribute('data-ds-dark-theme')).toBe(
        theme === 'dark' ? '' : null,
      );
      await page.setViewportSize({ width: size.width, height: size.height });
      const expectedWidth = size.width <= 720 ? size.width - 32 : Math.min(960, size.width * 0.6);
      const expectedHeight = size.width <= 720 ? size.height - 32 : size.height * 0.6;
      await expect
        .poll(async () => (await dialog.boundingBox())!.width)
        .toBeCloseTo(expectedWidth, 0);
      await expect
        .poll(async () => (await dialog.boundingBox())!.height)
        .toBeCloseTo(expectedHeight, 0);
      const close = dialog.getByRole('button', { name: 'Close', exact: true });
      await close.focus();
      await page.keyboard.press('Shift+Tab');
      expect(await dialog.evaluate((element) => element.contains(document.activeElement))).toBe(
        true,
      );
      await page.keyboard.press('Tab');
      await expect(close).toBeFocused();
      await expect(close).toBeInViewport({ ratio: 1 });
      expect(await close.evaluate((element) => getComputedStyle(element).boxShadow)).not.toBe(
        'none',
      );
      const title = dialog.locator('.modal__title');
      expect(await title.evaluate((element) => element.scrollWidth <= element.clientWidth)).toBe(
        true,
      );
      const titleBox = (await title.boundingBox())!;
      const closeBox = (await close.boundingBox())!;
      expect(titleBox.x + titleBox.width).toBeLessThanOrEqual(closeBox.x);
      expect(closeBox.width).toBeGreaterThanOrEqual(32);
      await page.screenshot({
        path: info.outputPath('sections-header.png'),
        animations: 'disabled',
      });

      const navigation = dialog.getByRole('navigation', { name: 'Configuration sections' });
      const activity = navigation.getByRole('button', { name: 'Activity', exact: true });
      await activity.focus();
      await expect(activity).toBeInViewport({ ratio: 1 });
      if (size.width <= 720)
        expect((await activity.boundingBox())!.height).toBeGreaterThanOrEqual(44);
      await page.keyboard.press('Enter');
      const receipt = dialog.getByText('No recipient delivery receipt recorded.', { exact: true });
      await receipt.scrollIntoViewIfNeeded();
      await expect(receipt).toBeInViewport({ ratio: 1 });
      await expect(close).toBeInViewport({ ratio: 1 });
      const inspector = dialog.locator('.channel-inspector');
      expect(await inspector.evaluate((element) => element.scrollTop)).toBeGreaterThan(0);
      expect(
        await inspector.evaluate((element) => element.scrollWidth <= element.clientWidth + 1),
      ).toBe(true);
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      );
      expect(await page.evaluate(() => getComputedStyle(document.body).overflow)).toBe('hidden');
      await page.screenshot({
        path: info.outputPath('sections-receipt.png'),
        animations: 'disabled',
      });
      await page.keyboard.press('Escape');
      await expect(dialog).toBeHidden();
      await expect(opener).toBeFocused();
      await longTitleOpener.click();
      const longDialog = page.getByRole('dialog', { name, exact: true });
      const longTitle = longDialog.locator('.modal__title');
      const longClose = longDialog.getByRole('button', { name: 'Close', exact: true });
      await expect(longDialog).toBeVisible();
      expect(
        await longTitle.evaluate((element) => element.scrollWidth <= element.clientWidth),
      ).toBe(true);
      expect((await longClose.boundingBox())!.width).toBeGreaterThanOrEqual(32);
      await expect(longClose).toBeInViewport({ ratio: 1 });
      await page.screenshot({
        path: info.outputPath('sections-long-title.png'),
        animations: 'disabled',
      });
      await longClose.click();
      await expect(longDialog).toBeHidden();
      await expect(longTitleOpener).toBeFocused();
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.writes).toEqual([]);
      expect(boundary.errors).toEqual([]);
      await info.attach('sections-layout.json', {
        body: JSON.stringify({
          theme,
          ...size,
          expectedWidth,
          expectedHeight,
          synthetic: true,
          nativeZoom: false,
        }),
        contentType: 'application/json',
      });
    });
  }
}

test('LOCAL a terminal gateway refusal stops automatic session reads until sign-in', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  await page.clock.install();
  let signedIn = false;
  let sessionReads = 0;
  let channelReads = 0;
  await page.route(`${baseURL}/auth/session`, (route) => route.fulfill({ json: cmaSession }));
  await page.route(`${baseURL}/auth/login?*`, (route) => {
    expect(new URL(route.request().url()).searchParams.get('return_to')).toBe('/event-gateway');
    signedIn = true;
    return route.fulfill({ status: 302, headers: { location: '/event-gateway' } });
  });
  await page.route(`${baseURL}/api/event-gateway/**`, (route) => {
    expect(route.request().method()).toBe('GET');
    const { pathname } = new URL(route.request().url());
    if (pathname.endsWith('/auth/session')) {
      sessionReads += 1;
      return signedIn
        ? route.fulfill({ json: gatewaySession })
        : route.fulfill({ status: 401, json: { detail: 'Authentication required' } });
    }
    expect(signedIn).toBe(true);
    if (pathname.endsWith('/v1/channels')) {
      channelReads += 1;
      return route.fulfill({ json: { items: [teamChannel], next_cursor: null } });
    }
    if (pathname.endsWith('/v1/channel-capabilities'))
      return route.fulfill({ json: channelCapabilities });
    expect(pathname).toMatch(/\/v1\/connections$/);
    return route.fulfill({ json: { items: [] } });
  });
  await page.goto('/agent-manager');
  await expect(page.getByRole('button', { name: 'Account menu for Local owner' })).toBeVisible();
  await page.getByRole('button', { name: 'Service: Agent Manager' }).click();
  await page.getByRole('menuitemradio', { name: 'Event Gateway' }).click();
  const signIn = page.getByRole('link', { name: 'Sign in with NyxID' });
  await expect(signIn).toBeVisible();
  await page.evaluate(() => {
    window.dispatchEvent(new Event('focus'));
    window.dispatchEvent(new Event('focus'));
  });
  await page.clock.runFor(135_000);
  expect(sessionReads).toBe(1);
  expect(channelReads).toBe(0);
  await expect(signIn).toBeVisible();

  await signIn.click();
  await expect(page.getByRole('tree', { name: 'Channels', exact: true })).toBeVisible();
  await expect.poll(() => channelReads).toBeGreaterThan(0);
  expect(sessionReads).toBeGreaterThan(1);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.writes).toEqual([]);
  expect(boundary.errors).toEqual([]);
});

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

test('LOCAL New channel announces the route and restores keyboard focus on Cancel', async ({
  page,
  baseURL,
}) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  await page.route(`${baseURL}/auth/session`, (route) => route.fulfill({ json: cmaSession }));
  await page.route(`${baseURL}/api/event-gateway/**`, (route) => {
    const { pathname } = new URL(route.request().url());
    if (pathname.endsWith('/auth/session')) return route.fulfill({ json: gatewaySession });
    if (pathname.endsWith('/v1/channel-capabilities'))
      return route.fulfill({ json: channelCapabilities });
    return route.fulfill({ json: { items: [] } });
  });
  await page.goto('/event-gateway');
  for (const width of [1440, 519]) {
    await page.setViewportSize({ width, height: 812 });
    const create = page
      .getByRole('region', { name: 'Channel workspace' })
      .getByRole('button', { name: 'New channel' });
    await create.focus();
    await page.keyboard.press('Enter');
    const heading = page.getByRole('heading', { name: 'New channel' });
    await expect(heading).toBeFocused();
    await expect(heading).toBeInViewport();
    await expect
      .poll(() => heading.evaluate((el) => getComputedStyle(el).boxShadow))
      .not.toBe('none');
    const cancel = page.getByRole('button', { name: 'Cancel', exact: true });
    await cancel.focus();
    await page.keyboard.press('Enter');
    await expect(create).toBeFocused();
  }
  const openNavigation = page.getByRole('button', { name: 'Show navigation', exact: true });
  await openNavigation.click();
  const drawerCreate = page
    .getByRole('complementary', { name: 'Application navigation' })
    .getByRole('button', { name: 'New channel', exact: true });
  await drawerCreate.focus();
  await page.keyboard.press('Enter');
  await expect(page.getByRole('heading', { name: 'New channel' })).toBeFocused();
  await expect(openNavigation).toBeVisible();
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(
    page
      .getByRole('region', { name: 'Channel workspace' })
      .getByRole('button', { name: 'New channel' }),
  ).toBeFocused();
});
