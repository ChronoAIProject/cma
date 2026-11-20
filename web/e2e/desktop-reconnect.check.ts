import { test, expect } from '@playwright/test';
import { LocalAgentV2, cursor } from './agent-v2.fixture';
import { desktopBoundary } from './desktop-rfb.fixture';

for (const status of [429, 409]) {
  test(`LOCAL desktop ${status} honors Retry-After across manual reconnect without a competing viewer`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalAgentV2();
    fixture.boundary.agent = { ...fixture.boundary.agent, desktop: true };
    let discovery = 0;
    try {
      await page.setViewportSize({ width: 1600, height: 1000 });
      // Keep decorative animation frames out of virtual retry time.
      await page.emulateMedia({ reducedMotion: 'reduce' });
      const anchor = Date.now();
      await page.clock.install({ time: anchor - 60_000 });
      await page.clock.pauseAt(anchor);
      await page.addInitScript(() => {
        Math.random = () => 0;
      });
      await fixture.route(page, baseURL!);
      const stream = await desktopBoundary(page);
      await page.route('**/api/v1/agents/agt_contract/desktop', (route) => {
        discovery++;
        return discovery === 1
          ? route.fulfill({
              status,
              headers: { 'Retry-After': '20' },
              json: {
                code: status === 409 ? 'desktop_busy' : 'rate_limited',
                detail: 'Busy',
              },
            })
          : route.fulfill({ json: { websocket_url: '/api/v1/agents/agt_contract/desktop/ws' } });
      });
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
      expect(
        await page.evaluate(() => matchMedia('(prefers-reduced-motion: reduce)').matches),
      ).toBe(true);
      await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
      const desktop = page.getByRole('region', { name: 'Machine desktop', exact: true });
      await expect(desktop.getByRole('status')).toContainText('Reconnecting');
      await page.getByRole('button', { name: 'Reconnect', exact: true }).click();
      await page.clock.runFor(19_999);
      expect(discovery).toBe(1);
      expect(stream.connections).toHaveLength(0);
      await page.clock.runFor(1);
      await expect.poll(() => discovery).toBe(2);
      await expect
        .poll(async () => {
          await page.clock.runFor(50);
          return desktop.getByRole('status').textContent();
        })
        .toContain('Agent paused · Desktop is idle');
      expect(stream.connections).toHaveLength(1);
      await expect(desktop).toHaveAttribute('data-controller', 'view');
      expect(fixture.writes).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}

for (const status of [404, 503]) {
  test(`LOCAL desktop ${status} stops recovery and explicit Reconnect recovers without writes`, async ({
    page,
    baseURL,
  }) => {
    test.setTimeout(60_000);
    const fixture = new LocalAgentV2();
    fixture.boundary.agent = { ...fixture.boundary.agent, desktop: true };
    let discovery = 0;
    let available = false;
    try {
      await page.setViewportSize({ width: status === 404 ? 390 : 1600, height: 1000 });
      // Keep decorative animation frames out of virtual retry time.
      await page.emulateMedia({ reducedMotion: 'reduce' });
      const anchor = Date.now();
      await page.clock.install({ time: anchor - 60_000 });
      await page.clock.pauseAt(anchor);
      await page.addInitScript(() => {
        Math.random = () => 0;
      });
      await fixture.route(page, baseURL!);
      const stream = await desktopBoundary(page);
      await page.route('**/api/v1/agents/agt_contract/desktop', (route) => {
        discovery++;
        return available
          ? route.fulfill({ json: { websocket_url: '/api/v1/agents/agt_contract/desktop/ws' } })
          : route.fulfill({
              status,
              json: {
                code: status === 404 ? 'desktop_unavailable' : 'desktop_runtime_unavailable',
                detail: 'Synthetic failure',
              },
            });
      });
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
      expect(
        await page.evaluate(() => matchMedia('(prefers-reduced-motion: reduce)').matches),
      ).toBe(true);
      await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
      const desktop = page.getByRole('region', { name: 'Machine desktop', exact: true });
      await expect.poll(() => discovery).toBe(1);
      if (status === 503) {
        await expect(desktop.getByRole('status')).toContainText('Reconnecting');
        for (const [index, delay] of [1000, 2000, 4000, 8000, 16000, 30000].entries()) {
          await page.clock.runFor(delay);
          await expect.poll(() => discovery).toBe(index + 2);
          // Flush the response before advancing to the next retry deadline.
          await expect(desktop.getByRole('status')).toContainText(
            index === 5 ? 'Could not reconnect' : 'Reconnecting',
          );
        }
      }
      const message =
        status === 404
          ? 'Desktop is currently unavailable. Reconnect when it becomes available.'
          : 'Could not reconnect automatically. Choose Reconnect to try again.';
      await expect(desktop.getByRole('status')).toHaveText(message);
      await expect(desktop.locator('.agent-desktop__loading')).toHaveText(message);
      await expect(desktop.locator('.agent-desktop__loading')).toHaveAttribute(
        'aria-hidden',
        'true',
      );
      await expect(desktop.locator('.agent-desktop__placeholder')).toHaveCount(0);
      await expect(
        desktop.getByRole('button', { name: 'Take control', exact: true }),
      ).toBeDisabled();
      // The unit regression also advances ten minutes with no remaining timers.
      await page.clock.runFor(10_000);
      expect(discovery).toBe(status === 404 ? 1 : 7);
      expect(stream.connections).toHaveLength(0);
      available = true;
      const reconnect = page.getByRole('button', { name: 'Reconnect', exact: true });
      await reconnect.focus();
      await reconnect.press('Enter');
      if (status === 503) {
        await page.clock.runFor(19_999);
        expect(discovery).toBe(7);
        expect(stream.connections).toHaveLength(0);
        await page.clock.runFor(1);
      }
      await expect.poll(() => discovery).toBe(status === 404 ? 2 : 8);
      await expect
        .poll(async () => {
          await page.clock.runFor(50);
          return desktop.getByRole('status').textContent();
        })
        .toContain('Agent paused · Desktop is idle');
      expect(stream.connections).toHaveLength(1);
      await expect(desktop).toHaveAttribute('data-controller', 'view');
      expect(stream.inputs).toEqual([]);
      expect(fixture.writes).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}
