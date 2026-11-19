import { test, expect } from '@playwright/test';
import { binding, LocalAgentV2 } from './agent-v2.fixture';
import { THINKING_NO_PROGRESS_THRESHOLD_MS } from '../src/agent/ThinkingProgress';

for (const theme of ['light', 'dark']) {
  test(`LOCAL ${theme}: terminal recovery and observed progress never resend work`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalAgentV2();
    fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
    fixture.append({ type: 'message', role: 'user', content: 'Synthetic accepted question' });
    try {
      await fixture.route(page, baseURL!);
      await page.clock.install();
      await page.goto('/agents/agt_contract');
      await expect(page.getByText('Thinking', { exact: true })).toBeVisible();
      await expect.poll(() => fixture.streams.size).toBe(1);
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      await page.clock.fastForward(THINKING_NO_PROGRESS_THRESHOLD_MS);
      const progress = page.getByText('No new activity observed here for 5 minutes.');
      await expect(progress).toBeVisible();
      await page.setViewportSize({ width: 390, height: 900 });
      await expect(progress).toBeVisible();
      expect(
        await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
      ).toBe(true);
      fixture.gap(1, 'reconnected');
      await expect(progress).toBeVisible();
      fixture.live('text_delta', 'Visible synthetic partial reply');
      await expect(
        page.getByText('Visible synthetic partial reply', { exact: true }),
      ).toBeVisible();
      await expect(progress).toHaveCount(0);
      fixture.gap(1);
      await expect(page.getByText('Refreshing live updates…', { exact: true })).toHaveCount(1);
      fixture.append({
        type: 'turn_failed',
        turn_sequence: 1,
        failure: { class: 'runtime', detail: 'Synthetic model failure' },
      });
      await expect(page.getByText('Agent run failed', { exact: true })).toBeVisible();
      await expect(page.getByText('Refreshing live updates…', { exact: true })).toHaveCount(0);
      await expect(page.getByText('Thinking', { exact: true })).toHaveCount(0);
      fixture.gap(null);
      fixture.append({ type: 'status_changed', from: 'working', to: 'ready' });
      fixture.gap(null, 'reconnected');
      fixture.gap(null);
      await expect(page.getByText('Incomplete live output', { exact: true })).toHaveCount(1);
      await expect(
        page.getByText('Visible synthetic partial reply', { exact: true }),
      ).toBeVisible();
      await expect(page.locator('.message-stream__live-recovery-slot')).toBeEmpty();
      await expect(page.locator('[data-streaming]')).toHaveCount(0);
      await page.reload();
      await expect(page.getByText('Agent run failed', { exact: true })).toBeVisible();
      await expect.poll(() => fixture.streams.size).toBe(1);
      fixture.gap(null);
      await expect(page.getByText('Refreshing live updates…', { exact: true })).toHaveCount(0);
      const next = fixture.items.length + 1;
      fixture.append(
        { type: 'turn_started', turn_sequence: next },
        {
          command_id: 'cmd_next',
          response_id: 'resp_next',
          turn_sequence: next,
        },
      );
      await expect(page.getByText('Thinking', { exact: true })).toBeVisible();
      fixture.gap(next);
      await expect(page.getByText('Refreshing live updates…', { exact: true })).toHaveCount(1);
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
