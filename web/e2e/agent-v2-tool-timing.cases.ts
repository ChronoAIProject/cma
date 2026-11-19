import { expect, test } from '@playwright/test';
import { binding, LocalAgentV2 } from './agent-v2.fixture';

for (const unknown of [false, true]) {
  test(`LOCAL tool timing preserves recorded intervals through streaming and reload (unknown=${unknown})`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalAgentV2();
    const start = '2026-11-05T14:33:00.000Z';
    const tools = [
      {
        id: 'command',
        title: 'Check fixture',
        kind: 'execute',
        end: '2026-11-05T14:33:00.550Z',
        duration: '0.6s',
      },
      {
        id: 'read',
        title: 'Read fixture',
        kind: 'read',
        end: '2026-11-05T14:33:00.020Z',
        duration: '<0.1s',
      },
      {
        id: 'search',
        title: 'Search fixture',
        kind: 'search',
        end: unknown ? start : '2026-11-05T14:33:00.550Z',
        duration: unknown ? 'Duration unavailable' : '0.6s',
      },
    ];
    const summary = `Ran 1 command · Read 1 file · Searched 1 time ${unknown ? 'Duration unavailable' : '1.1s'}`;
    try {
      await fixture.route(page, baseURL!);
      await page.goto('/agents/agt_contract');
      await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toBeVisible();
      await expect.poll(() => fixture.streams.size).toBe(1);
      fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
      for (const tool of tools)
        fixture.append(
          {
            type: 'tool_started',
            call_id: tool.id,
            title: tool.title,
            kind: tool.kind,
            status: 'running',
          },
          null,
          start,
        );
      await expect(page.locator('.tool-call__duration')).toHaveCount(0);
      for (const tool of tools)
        fixture.append(
          { type: 'tool_updated', call_id: tool.id, status: 'completed', title: null },
          null,
          tool.end,
        );
      fixture.append({ type: 'turn_completed', turn_sequence: 1 });
      const inspect = async () => {
        const group = page.getByRole('button', { name: summary, exact: true });
        await expect(group).toBeVisible();
        await group.click();
        await expect(page.locator('.agent-event--tool')).toHaveCount(3);
        for (const width of [1440, 390, 320]) {
          await page.setViewportSize({ width, height: 900 });
          await expect
            .poll(() => page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1))
            .toBe(true);
        }
        for (const tool of tools) {
          const row = page.getByRole('article', { name: `Tool call: ${tool.title}`, exact: true });
          await expect(row.getByText(tool.duration, { exact: true })).toBeVisible();
          await expect(row.getByRole('img', { name: 'Completed' })).toBeVisible();
        }
        await expect(
          page.locator('.tool-call__duration').filter({ hasText: /^0\.0s$/ }),
        ).toHaveCount(0);
      };
      await inspect();
      await page.reload();
      await inspect();
      expect(fixture.writes).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}

for (const application of ['Browser', 'Desktop']) {
  test(`LOCAL tool timing keeps ${application} step times readable when the group duration is unavailable`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalAgentV2();
    const start = '2026-11-05T14:33:00Z';
    const title =
      application === 'Browser' ? 'cma-browser navigate https://example.test' : 'xdotool click 1';
    fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
    for (const id of ['known', 'unknown']) {
      fixture.append(
        { type: 'tool_started', call_id: id, title, kind: 'execute', status: 'running' },
        null,
        start,
      );
      fixture.append(
        { type: 'tool_updated', call_id: id, title: null, status: 'completed' },
        null,
        id === 'known' ? '2026-11-05T14:33:02Z' : start,
      );
    }
    fixture.append({ type: 'turn_completed', turn_sequence: 1 });
    try {
      await fixture.route(page, baseURL!);
      await page.goto('/agents/agt_contract');
      const summary = page.getByRole('button', {
        name: `Checked it in ${application} · 2 steps Duration unavailable`,
        exact: true,
      });
      await summary.click();
      await expect(page.locator('.tool-call__screen')).toHaveCount(2);
      await expect(page.locator('.tool-call__screen time')).toHaveCount(2);
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 });
        await expect
          .poll(() =>
            page.locator('.tool-call__screen').evaluateAll((rows) =>
              rows.every((row) => {
                const literal = row.querySelector('.tool-call__literal')!.getBoundingClientRect();
                const time = row.querySelector('time')!.getBoundingClientRect();
                return literal.width > 0 && time.right <= row.getBoundingClientRect().right + 1;
              }),
            ),
          )
          .toBe(true);
      }
      expect(fixture.writes).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}
