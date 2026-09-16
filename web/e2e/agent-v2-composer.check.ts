import { test, expect } from '@playwright/test';
import { LocalAgentV2 } from './agent-v2.fixture';

for (const theme of ['light', 'dark']) {
  test(`LOCAL ${theme}: queue ordinary input during an active response without steering it`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    fixture.queued.enabled = true;
    fixture.append(
      { type: 'turn_started', turn_sequence: 1 },
      {
        response_id: 'resp_active',
        command_id: 'cmd_active',
        turn_sequence: 1,
      },
    );
    try {
      await fixture.route(page, baseURL!);
      await page.goto('/agents/agt_contract');
      const field = page.getByRole('textbox', { name: 'Message', exact: true });
      const queue = page.getByRole('button', { name: 'Queue message', exact: true });
      const stop = page.getByRole('button', { name: 'Stop response', exact: true });
      await expect(queue).toBeVisible();
      await expect(stop).toBeEnabled();
      await field.fill('Continue with this after the active response');
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 });
        for (const button of [queue, stop]) {
          await expect
            .poll(() =>
              button.evaluate((element) => {
                const box = element.getBoundingClientRect();
                return (
                  box.left >= 0 &&
                  box.right <= innerWidth &&
                  box.top >= 0 &&
                  box.bottom <= innerHeight
                );
              }),
            )
            .toBe(true);
        }
        await expect
          .poll(async () => {
            const [queueBox, stopBox] = await Promise.all([
              queue.boundingBox(),
              stop.boundingBox(),
            ]);
            if (queueBox === null || stopBox === null) return Number.POSITIVE_INFINITY;
            return Math.abs(queueBox.y + queueBox.height / 2 - (stopBox.y + stopBox.height / 2));
          })
          .toBeLessThan(1);
        await page.screenshot({
          path: info.outputPath(`queued-composer-${theme}-${width}.png`),
          animations: 'disabled',
        });
      }
      await field.press('Enter');
      await expect(field).toHaveValue('');
      await expect(
        page.getByRole('button', { name: 'Cancel queued message', exact: true }),
      ).toBeEnabled();
      expect(fixture.writes).toHaveLength(1);
      expect(fixture.writes[0]!.path).toBe('/api/v2/agents/agt_contract/responses');
      expect(fixture.writes[0]!.body).toEqual({
        input: 'Continue with this after the active response',
        stream: true,
      });
      await expect(page.getByRole('button', { name: /^Thinking/ })).toBeVisible();
      await expect(stop).toBeEnabled();
      fixture.append({
        type: 'tool_started',
        call_id: 'ongoing',
        title: 'Continue original work',
        kind: 'execute',
        status: 'running',
      });
      await expect(page.getByRole('button', { name: /^Thinking/ })).toContainText(
        'Continue original work',
      );
      await field.fill('Keep this next draft');
      await page.getByRole('button', { name: 'Cancel queued message', exact: true }).click();
      await expect(
        page
          .getByRole('status')
          .filter({ hasText: 'Your queued message was cancelled before it started.' }),
      ).toBeVisible();
      await expect(field).toHaveValue('Keep this next draft');
      await expect(stop).toBeEnabled();
      expect(fixture.writes.filter((write) => write.path.endsWith('/controls'))).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });
}
