import { mkdir } from 'node:fs/promises';
import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

for (const width of [1440, 390]) {
  test(`LOCAL final answer feedback is accessible and restored at ${width}px`, async ({
    page,
    baseURL,
  }) => {
    await page.setViewportSize({ width, height: 1000 });
    const boundary = new LocalStartup();
    const timestamp = '2026-11-03T01:00:00Z';
    boundary.events = [
      { seq: 1, appended_at: timestamp, kind: { type: 'turn_started', turn_sequence: 1 } },
      {
        seq: 2,
        appended_at: timestamp,
        kind: { type: 'owner_message', content: 'Review the result.' },
      },
      {
        seq: 3,
        appended_at: timestamp,
        kind: { type: 'agent_message', content: 'Checking the evidence.' },
      },
      {
        seq: 4,
        appended_at: timestamp,
        kind: { type: 'agent_message', content: 'The final answer is ready.' },
      },
      { seq: 5, appended_at: timestamp, kind: { type: 'turn_completed', turn_sequence: 1 } },
    ];
    await boundary.route(page, baseURL!);
    let vote: 'up' | 'down' | null = null;
    let loseResponse = false;
    await page.route('**/api/v1/agents/agt_contract/message-feedback?*', (route) =>
      route.fulfill({
        json: { items: vote === null ? [] : [{ message_id: 'event:4', vote }], next: null },
      }),
    );
    await page.route('**/api/v1/agents/agt_contract/messages/*/feedback', async (route) => {
      expect(route.request().method()).toBe('PUT');
      expect(decodeURIComponent(new URL(route.request().url()).pathname)).toMatch(
        /\/messages\/event:4\/feedback$/,
      );
      vote = (route.request().postDataJSON() as { vote: typeof vote }).vote;
      if (loseResponse) return route.abort('failed');
      return route.fulfill({ json: { message_id: 'event:4', vote } });
    });
    await page.goto('/agents/agt_contract');
    const reply = page.getByRole('article').filter({ hasText: 'The final answer is ready.' });
    const helpful = reply.getByRole('button', { name: 'Helpful', exact: true });
    await expect(helpful).toBeEnabled();
    await expect(page.getByRole('button', { name: 'Helpful', exact: true })).toHaveCount(1);
    await page.mouse.move(0, 0);
    if (width === 390) {
      await expect(reply.getByRole('group', { name: 'Message actions' })).toHaveCSS('opacity', '1');
      const bounds = (await helpful.boundingBox())!;
      expect(bounds.width).toBeGreaterThanOrEqual(44);
      expect(bounds.height).toBeGreaterThanOrEqual(44);
    } else {
      await expect(reply.getByRole('group', { name: 'Message actions' })).toHaveCSS('opacity', '0');
    }
    await reply.getByRole('button', { name: 'Copy content' }).focus();
    await page.keyboard.press('Tab');
    await expect(helpful).toBeFocused();
    await expect(reply.getByRole('group', { name: 'Message actions' })).toHaveCSS('opacity', '1');
    await page.keyboard.press('Enter');
    await expect(reply.getByRole('status')).toHaveText('Vote recorded');
    await expect(helpful).toHaveAttribute('aria-pressed', 'true');
    await page
      .getByRole('article')
      .filter({ hasText: 'Review the result.' })
      .getByRole('button', { name: 'Copy content' })
      .focus();
    await expect(helpful).not.toBeFocused();
    await page.mouse.move(0, 0);
    await expect(reply.getByRole('group', { name: 'Message actions' })).toHaveCSS('opacity', '1');
    await mkdir('/tmp/cma-ticket09-evidence', { recursive: true });
    await page.screenshot({
      path: `/tmp/cma-ticket09-evidence/after-${width}.png`,
      fullPage: true,
    });
    loseResponse = true;
    await reply.getByRole('button', { name: 'Needs improvement', exact: true }).click();
    await expect(reply.getByRole('button', { name: 'Retry vote' })).toBeVisible();
    await expect(helpful).toHaveAttribute('aria-pressed', 'true');
    await expect(helpful).toBeDisabled();
    await page.reload();
    await expect(
      reply.getByRole('button', { name: 'Needs improvement', exact: true }),
    ).toHaveAttribute('aria-pressed', 'true');
    await expect(helpful).toBeEnabled();
    loseResponse = false;
    await reply.getByRole('button', { name: 'Needs improvement', exact: true }).click();
    await expect(reply.getByRole('status')).toHaveText('Vote removed');
    await page.reload();
    await expect(helpful).toHaveAttribute('aria-pressed', 'false');
    await expect(
      reply.getByRole('button', { name: 'Needs improvement', exact: true }),
    ).toHaveAttribute('aria-pressed', 'false');
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}
