import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { desktopBoundary } from './desktop-rfb.fixture';
import { binding, cursor, LocalAgentV2 } from './agent-v2.fixture';
import { captureSingleBrowserFrame } from './screen-preview';
import {
  browser,
  policy,
  registerDesignCorrectionTaskCases,
  registerDesignCorrectionHomeCases,
} from './design-correction-home.cases';

registerDesignCorrectionTaskCases();

for (const width of [1440, 390]) {
  test(`LOCAL computer history at ${width}px folds raw records and preserves navigation`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    await page.setViewportSize({ width, height: 900 });
    await page.addInitScript(() => localStorage.setItem('cma:appearance', 'dark'));
    fixture.append({ type: 'turn_started', turn_sequence: 1 });
    for (const number of [1, 2, 3]) {
      fixture.append({
        type: 'tool_started',
        call_id: `command-${number}`,
        kind: 'execute',
        title: `printf operation-${number}`,
        status: 'in_progress',
      });
      fixture.append({
        type: 'tool_updated',
        call_id: `command-${number}`,
        title: null,
        status: 'completed',
        output: { text: `Recorded output ${number}`, truncated: false },
      });
    }
    fixture.append({ type: 'turn_completed', turn_sequence: 1 });
    await fixture.route(page, baseURL!);
    await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
      route.fulfill({
        json: {
          object: 'cma.agent.event_head',
          schema_version: 1,
          agent_id: 'agt_contract',
          incarnation: 'a'.repeat(64),
          through_cursor: cursor(fixture.items.length),
          active_turn: null,
        },
      }),
    );
    await policy(page, true);
    await browser(page, true);
    try {
      await page.goto('/agents/agt_contract');
      await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
      await page.getByRole('tab', { name: 'Operations' }).click();
      const output = page.locator('#computer-page-activity');
      for (const number of [1, 2, 3])
        await expect(output.getByText(`Recorded output ${number}`, { exact: true })).toBeVisible();
      await expect(output.getByRole('slider')).toHaveCount(0);
      await page.screenshot({ path: info.outputPath(`terminal-${width}.png`) });
      expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(
        true,
      );
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}

test('LOCAL shared desktop uses the same computer panel and releases control on tab changes', async ({
  page,
  baseURL,
}) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const boundary = new LocalStartup();
  boundary.agent = { ...boundary.agent, desktop: true };
  await boundary.route(page, baseURL!);
  const stream = await desktopBoundary(page);
  await page.goto('/agents/agt_contract');
  await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
  const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
  const desktop = computer.getByRole('region', { name: 'Machine desktop' });
  await expect(desktop.getByRole('status')).toContainText('Agent paused · Desktop is idle');
  await page.getByRole('textbox', { name: 'Message', exact: true }).fill('Retain this draft');
  await desktop.getByRole('button', { name: 'Take control' }).click();
  await expect(desktop.getByRole('button', { name: 'Return to Agent' })).toBeVisible();
  await computer.getByRole('tab', { name: 'Files' }).click();
  await expect(page.getByRole('region', { name: 'Machine desktop' })).toHaveCount(0);
  await computer.getByRole('tab', { name: 'Screen', exact: true }).click();
  await expect(desktop.getByRole('status')).toContainText('Agent paused · Desktop is idle');
  await expect(desktop.getByRole('button', { name: 'Take control' })).toBeVisible();
  expect(stream.connections).toHaveLength(2);
  await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toHaveValue(
    'Retain this draft',
  );
});

registerDesignCorrectionHomeCases();

for (const theme of ['light', 'dark']) {
  test(`LOCAL Screen follows browser and desktop operations in ${theme}, and answer timestamps reveal over answer prose`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalAgentV2();
    fixture.boundary.agent = { ...fixture.boundary.agent, desktop: true };
    fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
    fixture.append({
      type: 'message',
      role: 'assistant',
      content: 'A retained answer for hover verification.',
    });
    fixture.append({ type: 'turn_completed', turn_sequence: 1 });
    try {
      await page.setViewportSize({ width: 1440, height: 900 });
      await page.addInitScript((value) => localStorage.setItem('cma:appearance', value), theme);
      await fixture.route(page, baseURL!);
      await desktopBoundary(page);
      await browser(page);
      await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
        route.fulfill({
          json: {
            object: 'cma.agent.event_head',
            schema_version: 1,
            agent_id: 'agt_contract',
            incarnation: 'a'.repeat(64),
            through_cursor: cursor(fixture.items.length),
            active_turn: null,
          },
        }),
      );
      await page.goto('/agents/agt_contract');
      await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
      const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
      await expect(computer.getByRole('region', { name: 'Machine desktop' })).toBeVisible();
      const answer = page.getByRole('article', { name: 'Chrono Agent' });
      const time = answer.locator('.message__timestamp');
      await page.mouse.move(1400, 800);
      await expect.poll(() => time.evaluate((node) => getComputedStyle(node).opacity)).toBe('0');
      await answer.locator('.agent-content').hover();
      await expect.poll(() => time.evaluate((node) => getComputedStyle(node).opacity)).toBe('1');
      await page.mouse.move(1400, 800);
      await answer.getByRole('button', { name: 'Copy content' }).focus();
      await expect.poll(() => time.evaluate((node) => getComputedStyle(node).opacity)).toBe('1');
      await expect.poll(() => fixture.streams.size).toBeGreaterThan(0);
      fixture.append({
        type: 'tool_started',
        call_id: 'browser-step',
        kind: 'execute',
        title: 'cma-browser navigate https://example.com',
        status: 'completed',
      });
      await captureSingleBrowserFrame(page);
      await expect(computer.getByRole('region', { name: 'Machine desktop' })).toHaveCount(0);
      fixture.append({
        type: 'tool_started',
        call_id: 'desktop-step',
        kind: 'execute',
        title: 'xdotool click 1',
        status: 'completed',
      });
      await expect(computer.getByRole('region', { name: 'Machine desktop' })).toBeVisible();
      await expect(computer.locator('canvas:visible')).toHaveCount(1);
      await expect(page.getByAltText('Agent browser page')).toHaveCount(0);
      await expect(page.getByRole('button', { name: /Shared desktop|Agent browser/ })).toHaveCount(
        0,
      );
      expect(fixture.writes).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}

test.describe('LOCAL coarse-pointer answer footer', () => {
  test.use({ hasTouch: true });
  test('keeps the relative time visible without hovering', async ({ page, baseURL }) => {
    const fixture = new LocalAgentV2();
    fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
    fixture.append({ type: 'message', role: 'assistant', content: 'A retained touch answer.' });
    fixture.append({ type: 'turn_completed', turn_sequence: 1 });
    try {
      await fixture.route(page, baseURL!);
      await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
        route.fulfill({
          json: {
            object: 'cma.agent.event_head',
            schema_version: 1,
            agent_id: 'agt_contract',
            incarnation: 'a'.repeat(64),
            through_cursor: cursor(fixture.items.length),
            active_turn: null,
          },
        }),
      );
      await page.goto('/agents/agt_contract');
      expect(await page.evaluate(() => matchMedia('(pointer: coarse)').matches)).toBe(true);
      const time = page
        .getByRole('article', { name: 'Chrono Agent' })
        .locator('.message__timestamp');
      await expect(time).toHaveAttribute('datetime', fixture.items[1]!.appended_at);
      await expect.poll(() => time.evaluate((node) => getComputedStyle(node).opacity)).toBe('1');
    } finally {
      await fixture.close(page);
    }
  });
});
