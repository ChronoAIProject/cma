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
