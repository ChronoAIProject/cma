import { test, expect, type Page } from '@playwright/test';
import { binding, LocalAgentV2 } from './agent-v2.fixture';
import { THINKING_NO_PROGRESS_THRESHOLD_MS } from '../src/agent/ThinkingProgress';

for (const theme of ['light', 'dark']) {
  test(`LOCAL ${theme}: negotiated approval wait survives reload and link return without approving or resending`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    fixture.externalApprovals = true;
    fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
    fixture.append({ type: 'message', role: 'user', content: 'Synthetic approval task' });
    fixture.append({ type: 'message', role: 'assistant', content: 'Synthetic retained progress' });
    const request = {
      type: 'external_approval_requested',
      approval_id: 'ea_browser_fixture',
      provider: 'nyxid',
      turn_sequence: 1,
      revision: 1,
      waiting_since: fixture.started,
      deadline_at: new Date(Date.parse(fixture.started) + 300_000).toISOString(),
      approve_url: 'https://identity.example.test/assistant/approvals',
    };
    fixture.append(request);
    try {
      await fixture.route(page, baseURL!);
      await page.context().route('https://identity.example.test/**', (route) =>
        route.fulfill({
          contentType: 'text/html',
          body: '<title>Synthetic approval inbox</title><p>Review fixture</p>',
        }),
      );
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.clock.install();
      await page.goto('/agents/agt_contract');
      const wait = page.getByRole('heading', { name: 'Waiting for your approval', exact: true });
      const review = page.getByRole('link', { name: 'Review and approve', exact: true });
      await expect(wait).toBeVisible();
      await expect(review).toBeVisible();
      await expect(page.locator('.approval .character-avatar')).toHaveAttribute(
        'data-tone',
        'warning',
      );
      await expect(page.locator('.thinking')).toHaveCount(0);
      await expect(page.getByRole('heading', { name: 'Working on it', exact: true })).toHaveCount(
        0,
      );
      await expect(page.getByRole('button', { name: 'Allow', exact: true })).toHaveCount(0);
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      for (const width of [1440, 390]) {
        await settledViewport(page, width);
        await expect(review).toBeVisible();
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
        ).toBe(true);
        await page.screenshot({
          path: info.outputPath(`approval-${theme}-${width}.png`),
          animations: 'disabled',
        });
      }
      const popupPromise = page.waitForEvent('popup');
      await review.click();
      const popup = await popupPromise;
      await expect(popup).toHaveURL(request.approve_url);
      await popup.close();
      await page.bringToFront();
      await page.clock.fastForward(600_000);
      await expect(wait).toBeVisible();
      await expect(page.locator('.approval time')).toHaveAttribute('datetime', request.deadline_at);
      expect(fixture.writes).toEqual([]);
      await page.reload();
      await expect(wait).toBeVisible();
      await expect.poll(() => fixture.streams.size).toBe(1);
      fixture.append({
        type: 'external_approval_resolved',
        approval_id: request.approval_id,
        provider: 'nyxid',
        turn_sequence: 1,
        revision: 2,
        resolution: 'approved',
      });
      await expect(wait).toHaveCount(0);
      await expect(review).toHaveCount(0);
      await expect(page.getByText('Thinking', { exact: true })).toBeVisible();
      fixture.append({ ...request, revision: 3 });
      await expect(wait).toHaveCount(0);
      fixture.append({ ...request, approval_id: 'ea_browser_second' });
      await expect(wait).toBeVisible();
      fixture.append({
        type: 'turn_failed',
        turn_sequence: 1,
        failure: { class: 'approval_timeout', detail: 'Synthetic server terminal' },
      });
      await expect(
        page.getByText('The approval wait timed out. This turn has ended.', { exact: true }),
      ).toBeVisible();
      fixture.gap(1, 'reconnected');
      fixture.append({ ...request, revision: 4 });
      await expect(page.locator('.thinking')).toHaveCount(0);
      await expect(review).toHaveCount(0);
      await page.reload();
      await expect(wait).toHaveCount(0);
      await expect(page.getByText('Synthetic retained progress', { exact: true })).toBeVisible();
      expect(fixture.writes).toEqual([]);
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });

  for (const policy of ['read-only', 'missing-origin', 'unsafe-url']) {
    test(`LOCAL ${theme}: external approval remains visible without an unauthorized ${policy} link`, async ({
      page,
      baseURL,
    }) => {
      const fixture = new LocalAgentV2();
      fixture.externalApprovals = true;
      fixture.approvalReviewAllowed = policy !== 'read-only';
      fixture.approvalPolicyAvailable = policy !== 'missing-origin';
      fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
      fixture.append({
        type: 'external_approval_requested',
        approval_id: 'ea_restricted_fixture',
        provider: 'nyxid',
        turn_sequence: 1,
        revision: 1,
        waiting_since: fixture.started,
        deadline_at: new Date(Date.parse(fixture.started) + 300_000).toISOString(),
        approve_url:
          policy === 'unsafe-url'
            ? 'https://untrusted.example.test/assistant/approvals'
            : 'https://identity.example.test/assistant/approvals',
      });
      try {
        await fixture.route(page, baseURL!);
        await page.goto('/agents/agt_contract');
        await expect(
          page.getByRole('heading', { name: 'Waiting for your approval' }),
        ).toBeVisible();
        await expect(page.getByRole('link', { name: 'Review and approve' })).toHaveCount(0);
        await expect(page.getByRole('button', { name: 'Allow', exact: true })).toHaveCount(0);
        await expect(page.locator('.thinking')).toHaveCount(0);
        await expect(
          page.getByText(
            policy === 'read-only'
              ? 'Waiting for an authorized participant to review this request in NyxID.'
              : 'The approval review link is unavailable. The turn is still waiting for approval.',
          ),
        ).toBeVisible();
        expect(fixture.writes).toEqual([]);
        expect(fixture.errors).toEqual([]);
      } finally {
        await fixture.close(page);
      }
    });
  }

  test(`LOCAL ${theme}: changed stream policy clears the link until a fresh baseline`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalAgentV2();
    fixture.externalApprovals = true;
    fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
    fixture.append({
      type: 'external_approval_requested',
      approval_id: 'ea_policy_fixture',
      provider: 'nyxid',
      turn_sequence: 1,
      revision: 1,
      waiting_since: fixture.started,
      deadline_at: new Date(Date.parse(fixture.started) + 300_000).toISOString(),
      approve_url: 'https://identity.example.test/assistant/approvals',
    });
    try {
      await fixture.route(page, baseURL!);
      await page.goto('/agents/agt_contract');
      const review = page.getByRole('link', { name: 'Review and approve' });
      await expect(review).toBeVisible();
      await expect.poll(() => fixture.streams.size).toBe(1);
      fixture.approvalReviewAllowed = false;
      fixture.endTails();
      await expect(review).toHaveCount(0);
      await expect(page.getByText(/Approval access changed/)).toBeVisible();
      await page.reload();
      await expect(page.getByRole('heading', { name: 'Waiting for your approval' })).toBeVisible();
      await expect(review).toHaveCount(0);
      await expect(
        page.getByText('Waiting for an authorized participant to review this request in NyxID.'),
      ).toBeVisible();
      expect(fixture.writes).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });

  test(`LOCAL ${theme}: server timeout preserves history and prepares a new input only after review`, async ({
    page,
    baseURL,
  }, info) => {
    const fixture = new LocalAgentV2();
    fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
    fixture.append({ type: 'message', role: 'user', content: 'Synthetic original input' });
    try {
      await fixture.route(page, baseURL!);
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.clock.install();
      await page.goto('/agents/agt_contract');
      const waiting = page.getByText(/Still waiting for this turn to finish/);
      await expect(waiting).toBeVisible();
      await expect(waiting.locator('time')).toHaveAttribute('datetime', fixture.started);
      await expect.poll(() => fixture.streams.size).toBe(1);
      await page.clock.fastForward(60 * 60_000);
      fixture.gap(1, 'reconnected');
      await expect(waiting.locator('time')).toHaveAttribute('datetime', fixture.started);
      await expect(page.getByText('Agent run failed', { exact: true })).toHaveCount(0);
      await expect(page.getByText('Waiting for your approval', { exact: true })).toHaveCount(0);
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      for (const width of [1440, 390]) {
        await settledViewport(page, width);
        await expect(waiting).toBeVisible();
        await page.screenshot({
          path: info.outputPath(`waiting-${theme}-${width}.png`),
          animations: 'disabled',
        });
      }
      await page.reload();
      await expect(waiting.locator('time')).toHaveAttribute('datetime', fixture.started);
      await expect.poll(() => fixture.streams.size).toBe(1);
      fixture.append({
        type: 'message',
        role: 'assistant',
        content: 'Synthetic retained partial output',
      });
      fixture.append({
        type: 'turn_failed',
        turn_sequence: 1,
        failure: {
          class: 'agent_turn_timeout',
          detail: 'Synthetic detail must not select the copy',
        },
      });
      const terminal = page.getByText('This turn reached its time limit and has ended.', {
        exact: true,
      });
      await expect(terminal).toBeVisible();
      fixture.gap(1, 'reconnected');
      fixture.gap(null);
      await expect(waiting).toHaveCount(0);
      await expect(page.locator('.thinking')).toHaveCount(0);
      await expect(page.locator('.message-stream__live-recovery-slot')).toBeEmpty();
      await expect(
        page.getByText('Synthetic retained partial output', { exact: true }),
      ).toBeVisible();
      await page.reload();
      await expect(terminal).toBeVisible();
      await expect(page.getByText(/Repeating a task may repeat effects/)).toBeVisible();
      const retry = page.getByRole('button', { name: 'Review and retry', exact: true });
      await expect(retry).toBeVisible();
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      for (const width of [1440, 390]) {
        await settledViewport(page, width);
        await expect(terminal).toBeVisible();
        expect(
          await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
        ).toBe(true);
        await page.screenshot({
          path: info.outputPath(`timeout-${theme}-${width}.png`),
          animations: 'disabled',
        });
      }
      await retry.click();
      const field = page.getByRole('textbox', { name: 'Message', exact: true });
      await expect(field).toHaveValue('Synthetic original input');
      await expect(field).toBeFocused();
      expect(fixture.writes).toEqual([]);
      await page.getByRole('button', { name: 'Send message', exact: true }).click();
      await expect.poll(() => fixture.writes.length).toBe(1);
      expect(fixture.writes[0]!.body).toEqual({ input: 'Synthetic original input', stream: true });
      expect(fixture.writes[0]!.key).toBeTruthy();
      expect(fixture.writes[0]!.path).toBe('/api/v2/agents/agt_contract/responses');
      expect(fixture.boundary.writes).toEqual([]);
      expect(fixture.boundary.unexpected).toEqual([]);
      expect(fixture.boundary.errors).toEqual([]);
      expect(fixture.errors).toEqual([]);
    } finally {
      await fixture.close(page);
    }
  });

  test(`LOCAL ${theme}: terminal recovery and observed progress never resend work`, async ({
    page,
    baseURL,
  }) => {
    const fixture = new LocalAgentV2();
    fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
    fixture.append({ type: 'message', role: 'user', content: 'Synthetic accepted question' });
    try {
      await fixture.route(page, baseURL!);
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await page.clock.install();
      await page.goto('/agents/agt_contract');
      await expect(page.getByText('Waiting for a response', { exact: true })).toBeVisible();
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
      await expect(page.getByText('Waiting for a response', { exact: true })).toHaveCount(0);
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
      await expect(page.getByText('Waiting for a response', { exact: true })).toBeVisible();
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

async function settledViewport(page: Page, width: number) {
  await page.setViewportSize({ width, height: 900 });
  await page.evaluate(() => document.fonts.ready);
  await expect
    .poll(async () => {
      await page.clock.runFor(100);
      const box = await page.locator('.agent-surface').boundingBox();
      return box ? box.x + box.width : Infinity;
    })
    .toBeLessThanOrEqual(width + 1);
  await page.clock.runFor(500);
}
