import { writeFileSync } from 'node:fs';
import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import {
  channelActivityFrame,
  channelCapabilities,
  cmaSession,
  gatewaySession,
  teamChannel,
} from '../build/fixtures/eventGateway';

test('LOCAL channel map preserves node drafts and renders every producer and consumer in both themes', async ({
  page,
  baseURL,
}, info) => {
  const themeEvidence: Record<string, unknown>[] = [];
  const capture = async (file: string, theme: 'light' | 'dark') => {
    const expected = {
      darkAttribute: theme === 'dark',
      colorScheme: theme,
      pageBackground: theme === 'dark' ? 'rgb(11, 11, 12)' : 'rgb(255, 255, 255)',
      canvasBackground: theme === 'dark' ? 'rgb(24, 24, 27)' : 'rgb(247, 247, 248)',
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
  await page.route('**/auth/session', (route) => route.fulfill({ json: cmaSession }));
  await page.route('**/api/v1/bots?**', (route) =>
    route.fulfill({ json: { items: [], next_after: null } }),
  );
  let writes = 0;
  await page.route('**/api/event-gateway/**', async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (route.request().method() !== 'GET') writes++;
    if (path.endsWith('/auth/session')) return route.fulfill({ json: gatewaySession });
    if (path.endsWith('/v1/channel-capabilities'))
      return route.fulfill({ json: channelCapabilities });
    if (path.endsWith('/v1/connections')) return route.fulfill({ json: { items: [] } });
    if (path.endsWith('/activity/stream'))
      return route.fulfill({
        contentType: 'text/event-stream',
        body: channelActivityFrame,
      });
    if (path.endsWith('/v1/channels'))
      return route.fulfill({ json: { items: [teamChannel], next_cursor: null } });
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
  await map.getByRole('button', { name: /^Toolsets: Event tools/ }).click();
  await expect(page.getByText('readEventContext', { exact: true })).toBeVisible();
  const otherTools = page.getByText('Tools requiring setup (2)');
  await expect(otherTools).toBeVisible();
  await expect(page.getByText('prepareFormReply', { exact: true })).not.toBeVisible();
  await capture('channel-toolsets-conditional-light-1600.png', 'light');
  await otherTools.click();
  await expect(
    page.getByText('Requires a direct Discord or Lark / Feishu source', { exact: true }),
  ).toBeVisible();
  await expect(page.getByText('Requires a direct WhatsApp source')).toBeVisible();
  await capture('channel-toolsets-other-light-1600.png', 'light');
  await otherTools.click();
  await expect(
    page
      .getByRole('navigation', { name: 'Configuration sections' })
      .getByRole('button', { name: 'Event tools', exact: true }),
  ).toBeVisible();
  const toolNavigation = page.getByRole('navigation', { name: 'Configuration sections' });
  const toolPanel = page.locator('.modal:not([hidden]) .channel-inspector');
  const connectedAgents = page.getByRole('group', { name: 'Connected agents', exact: true });
  for (const reducedMotion of ['no-preference', 'reduce'] as const) {
    await page.emulateMedia({ reducedMotion });
    await expect(page.locator('.modal:not([hidden])')).toHaveCSS('opacity', '1');
    await toolNavigation.getByRole('button', { name: 'Connected agents' }).click();
    // Exercise the real browser's clamped last destination: its heading cannot
    // align with the scroll-spy's top threshold even after scrolling finishes.
    await expect
      .poll(() =>
        connectedAgents.evaluate((section) => {
          const panel = section.closest('.channel-inspector')!;
          return (
            panel.scrollHeight > panel.clientHeight + 1 &&
            panel.scrollTop >= panel.scrollHeight - panel.clientHeight - 1 &&
            section.getBoundingClientRect().top > panel.getBoundingClientRect().top + 32
          );
        }),
      )
      .toBe(true);
    await expect(toolNavigation.getByRole('button', { name: 'Connected agents' })).toHaveAttribute(
      'aria-current',
      'location',
    );
    await expect(connectedAgents).toBeFocused();
    if (reducedMotion === 'no-preference')
      await page.screenshot({
        path: info.outputPath('channel-toolsets-clamped-last.png'),
        animations: 'disabled',
      });
    // Native wheel input must release the navigation intent and resume the
    // actual scroll position's highlight in either motion preference.
    await toolPanel.hover();
    await page.mouse.wheel(0, -10000);
    await expect.poll(() => toolPanel.evaluate((panel) => panel.scrollTop)).toBe(0);
    await expect(toolNavigation.getByRole('button', { name: 'Overview' })).toHaveAttribute(
      'aria-current',
      'location',
    );
    await page.mouse.wheel(0, 10000);
    await expect(toolNavigation.getByRole('button', { name: 'Connected agents' })).toHaveAttribute(
      'aria-current',
      'location',
    );
    await connectedAgents.press('Home');
    await expect.poll(() => toolPanel.evaluate((panel) => panel.scrollTop)).toBe(0);
    await expect(toolNavigation.getByRole('button', { name: 'Overview' })).toHaveAttribute(
      'aria-current',
      'location',
    );
  }
  await page.getByRole('button', { name: 'Close', exact: true }).click();
  await page.setViewportSize({ width: 1600, height: 5000 });
  await map.getByRole('button', { name: /^Toolsets: Event tools/ }).click();
  await expect(page.locator('.modal:not([hidden])')).toHaveCSS('opacity', '1');
  await expect
    .poll(() => toolPanel.evaluate((panel) => panel.scrollHeight <= panel.clientHeight + 1))
    .toBe(true);
  // A fitting panel is also at its bottom, but must open on Overview and retain
  // an explicit selection even though no scroll event is needed to reveal it.
  await expect(toolNavigation.getByRole('button', { name: 'Overview' })).toHaveAttribute(
    'aria-current',
    'location',
  );
  await toolNavigation.getByRole('button', { name: 'Event tools', exact: true }).click();
  await expect(
    toolNavigation.getByRole('button', { name: 'Event tools', exact: true }),
  ).toHaveAttribute('aria-current', 'location');
  await toolNavigation.getByRole('button', { name: 'Overview' }).click();
  await expect(toolNavigation.getByRole('button', { name: 'Overview' })).toHaveAttribute(
    'aria-current',
    'location',
  );
  await page.getByRole('button', { name: 'Close', exact: true }).click();
  await page.setViewportSize({ width: 1600, height: 1000 });
  await page.emulateMedia({ reducedMotion: 'no-preference' });
  await map.getByRole('button', { name: 'Producer: Telegram (NyxID relay)' }).click();
  await expect(page.getByLabel('Bot username', { exact: true })).toHaveValue('retained_bot');
  await page.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(map.getByText('Browse delivery nodes')).toHaveCount(0);
  await expect(map.locator('.react-flow__edge-path[marker-end]')).toHaveCount(6);
  await expect(map.locator('.delivery-flow__tool-catalog')).toHaveCount(2);
  await expect(
    map.locator('.delivery-flow__tool-catalog .react-flow__edge-path[marker-end]'),
  ).toHaveCount(0);
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
      const height = page.viewportSize()!.height;
      const expectedWidth = width <= 720 ? width - 32 : Math.min(960, width * 0.6);
      const expectedHeight = width <= 720 ? height - 32 : height * 0.6;
      expect(bounds!.width).toBeCloseTo(expectedWidth, 0);
      expect(bounds!.height).toBeCloseTo(expectedHeight, 0);
      expect(bounds!.x).toBeCloseTo((width - expectedWidth) / 2, 0);
      expect(bounds!.y).toBeCloseTo((height - expectedHeight) / 2, 0);
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
  await page.getByRole('button', { name: 'New channel', exact: true }).click();
  const creation = page.locator('.gateway-detail--create');
  for (const width of [1600, 390]) {
    await page.setViewportSize({ width, height: 1000 });
    await expect(creation.getByRole('heading', { name: 'New channel', exact: true })).toBeVisible();
    const inset = width <= 600 ? 16 : 24;
    const layout = await creation.evaluate((element) => {
      const bounds = element.getBoundingClientRect();
      const header = element.querySelector('header')!.getBoundingClientRect();
      const form = element.querySelector('form')!.getBoundingClientRect();
      const input = element.querySelector('.input')!.getBoundingClientRect();
      const actions = [...element.querySelectorAll('button')].filter((button) =>
        ['Create channel', 'Cancel'].includes(button.textContent ?? ''),
      );
      return {
        headerInset: header.left - bounds.left,
        formInset: form.left - bounds.left,
        formRightInset: bounds.right - form.right,
        inputInset: input.left - bounds.left,
        actionInsets: actions.map((action) => action.getBoundingClientRect().left - bounds.left),
        overflow: element.scrollWidth - element.clientWidth,
      };
    });
    expect(layout.headerInset).toBeCloseTo(inset, 0);
    expect(layout.formInset).toBeCloseTo(inset, 0);
    expect(layout.formRightInset).toBeCloseTo(inset, 0);
    expect(layout.inputInset).toBeCloseTo(inset, 0);
    expect(layout.actionInsets).toEqual([inset, inset]);
    expect(layout.overflow).toBeLessThanOrEqual(1);
    await creation.locator('header').scrollIntoViewIfNeeded();
    await page.screenshot({
      path: info.outputPath(`new-channel-dark-${width}.png`),
      animations: 'disabled',
    });
  }
  expect(writes).toBe(0);
  expect(boundary.errors).toEqual([]);
});
