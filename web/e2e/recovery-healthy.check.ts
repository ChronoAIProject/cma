import { test, expect } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import { ReplicaFixture } from './replica-recovery.fixture';
import { binding } from './agent-v2.fixture';

const enabled = process.env.CMA_RECOVERY_ENABLED === 'true';
for (const action of ['send', 'cancel'] as const) {
  test(`LOCAL healthy ${action} critical path automatic ${enabled}`, async ({
    context,
    baseURL,
  }, info) => {
    const samples: { requestMs: number; visibleMs: number }[] = [];
    for (let sample = 0; sample < 10; sample += 1) {
      const fixture = new ReplicaFixture();
      const page = await context.newPage();
      if (action === 'cancel') fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
      try {
        await fixture.route(page, baseURL!);
        await page.goto('/agents/agt_contract');
        const input = page.getByRole('textbox', { name: 'Message', exact: true });
        const stop = page.getByRole('button', { name: 'Stop response', exact: true });
        await expect(input).toBeVisible();
        await expect.poll(() => fixture.streams.size).toBe(1);
        if (action === 'send') await input.fill('Healthy request');
        else await expect(stop).toBeEnabled();
        const start = performance.now();
        await (
          action === 'send' ? page.getByRole('button', { name: 'Send message', exact: true }) : stop
        ).click();
        await expect.poll(() => fixture.writes.length, { intervals: [10, 25, 50] }).toBe(1);
        if (action === 'send') await expect(page.locator('.message--user')).toContainText('Sent');
        else {
          // Fixture terminal publication is explicit and included in the timing;
          // a control receipt alone must not hide the active response.
          await expect(stop).toBeVisible();
          fixture.append({ type: 'turn_cancelled', turn_sequence: 1 });
          await expect(stop).toHaveCount(0);
        }
        const visibleMs = performance.now() - start;
        samples.push({ requestMs: fixture.writes[0]!.at - start, visibleMs });
        expect(visibleMs).toBeLessThanOrEqual(1000);
        expect(fixture.effects.size).toBe(1);
        expect(fixture.writes).toHaveLength(1);
        expect(fixture.errors).toEqual([]);
        expect(fixture.base.errors).toEqual([]);
      } finally {
        await fixture.close(page);
      }
    }
    await writeFile(
      info.outputPath('healthy.json'),
      JSON.stringify(
        {
          source: process.env.CMA_RECOVERY_SOURCE,
          enabled,
          action,
          samples,
          scope:
            'Local compiled Chrome and synthetic HTTP custody; Playwright click-start includes actionability; not production or distributed latency qualification',
        },
        null,
        2,
      ),
    );
  });
}
