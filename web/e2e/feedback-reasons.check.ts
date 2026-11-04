import { mkdir } from 'node:fs/promises';
import { expect, test } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import type { FeedbackDetails, FeedbackReasonRequest } from '../src/api/feedbackTypes';

for (const width of [1440, 390]) {
  test(`LOCAL independent feedback reasons and recovery at ${width}px`, async ({
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
    await page.route('**/api/v1/agents/agt_contract/files/tree**', (route) =>
      route.fulfill({ json: { path: '', entries: [] } }),
    );
    const unsupported = {
      state: 'unsupported',
      reason: 'not_supported_by_contract',
      scope: 'none',
      writable: false,
    };
    await page.route('**/api/v1/agents/agt_contract/environment', (route) =>
      route.fulfill({
        json: {
          agent_id: 'agt_contract',
          policy_revision: 1,
          runtime_incarnation: 'b'.repeat(64),
          revalidate_at: null,
          files: {
            state: 'available',
            reason: 'authorized_mounted_home',
            scope: 'mounted_home',
            writable: true,
          },
          operation_output: {
            state: 'available',
            reason: 'recorded_output_only',
            scope: 'agent_turn_history',
            writable: false,
          },
          browser: unsupported,
          replay: unsupported,
          control: unsupported,
        },
      }),
    );
    let state: FeedbackDetails = {
      viewer_id: 'local-owner',
      agent_id: 'agt_contract',
      message_id: 'event:4',
      vote: null,
      version: 0,
      reasons: [],
      comment: '',
      updated_at: null,
      request_id: null,
    };
    const writes: FeedbackReasonRequest[] = [];
    let release: (() => void) | undefined;
    let hold = false;
    let lose = false;
    await page.route('**/api/v1/agents/agt_contract/message-feedback?*', (route) =>
      route.fulfill({
        json: {
          items: state.vote ? [{ message_id: 'event:4', vote: state.vote }] : [],
          next: null,
        },
      }),
    );
    await page.route('**/api/v1/agents/agt_contract/messages/*/feedback', (route) => {
      if (route.request().method() === 'GET') return route.fulfill({ json: state });
      const { vote } = route.request().postDataJSON();
      state = {
        ...state,
        vote,
        version: state.version + 1,
        reasons: [],
        comment: '',
        request_id: null,
        updated_at: timestamp,
      };
      return route.fulfill({ json: { message_id: 'event:4', vote } });
    });
    await page.route('**/api/v1/agents/agt_contract/messages/*/feedback/reasons', async (route) => {
      const body = route.request().postDataJSON() as FeedbackReasonRequest;
      writes.push(body);
      if (hold)
        await new Promise<void>((resolve) => {
          release = resolve;
        });
      if (state.request_id !== body.request_id) {
        if (state.vote !== 'down' || state.version !== body.expected_version)
          return route.fulfill({ status: 409, json: {} });
        state = { ...state, ...body, version: state.version + 1, updated_at: timestamp };
      }
      if (lose) return route.abort('failed');
      return route.fulfill({ json: state });
    });
    await mkdir('/tmp/cma-ticket10-evidence', { recursive: true });
    const screenshot = (stage: string) =>
      page.screenshot({ path: `/tmp/cma-ticket10-evidence/${stage}-${width}.png`, fullPage: true });
    await page.goto('/agents/agt_contract');
    const reply = page.getByRole('article').filter({ hasText: 'The final answer is ready.' });
    const helpful = reply.getByRole('button', { name: 'Helpful', exact: true });
    const down = reply.getByRole('button', { name: 'Needs improvement', exact: true });
    await expect(helpful).toBeEnabled();
    await screenshot('default');
    await helpful.focus();
    await helpful.click();
    await expect(helpful).toHaveAttribute('aria-pressed', 'true');
    await screenshot('helpful');
    await down.click();
    const panel = reply.getByRole('region', { name: 'Reasons for improvement' });
    const save = panel.getByRole('button', { name: 'Save reasons', exact: true });
    await expect(save).toBeEnabled();
    expect(state.vote).toBe('down');
    const inaccurate = panel.getByRole('checkbox', { name: 'Inaccurate content' });
    await inaccurate.focus();
    await page.keyboard.press('Space');
    await panel.getByRole('checkbox', { name: 'Incomplete answer' }).check();
    const details = panel.getByRole('textbox', { name: 'Additional details (optional)' });
    await details.fill('Please check the evidence and include the missing steps.');
    await panel.getByRole('button', { name: 'Close', exact: true }).click();
    await expect(panel).toHaveCount(0);
    await reply.getByRole('button', { name: 'Edit reasons' }).click();
    await expect(details).toHaveValue('Please check the evidence and include the missing steps.');
    await expect(inaccurate).toBeChecked();
    await screenshot('expanded');
    if (width === 390) {
      const bounds = (await inaccurate.locator('..').boundingBox())!;
      expect(bounds.height).toBeGreaterThanOrEqual(44);
      expect((await save.boundingBox())!.height).toBeGreaterThanOrEqual(44);
    }
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
    ).toBe(true);
    hold = true;
    lose = true;
    await save.click();
    await expect(panel.getByText('Saving reasons…')).toBeVisible();
    await expect(helpful).toBeEnabled();
    await screenshot('saving');
    await expect.poll(() => Boolean(release)).toBe(true);
    release!();
    await expect(panel.getByRole('button', { name: 'Retry reasons' })).toBeEnabled();
    await screenshot('failed');
    hold = false;
    lose = false;
    await panel.getByRole('button', { name: 'Retry reasons' }).click();
    await expect(panel.getByText('Reasons saved')).toBeVisible();
    expect(writes).toHaveLength(2);
    expect(writes[1]).toEqual(writes[0]);
    await screenshot('saved');
    await page.reload();
    await reply.getByRole('button', { name: 'Edit reasons' }).click();
    await expect(inaccurate).toBeChecked();
    await expect(details).toHaveValue('Please check the evidence and include the missing steps.');
    await details.fill('🦀'.repeat(501));
    await expect(details).toHaveAttribute('aria-invalid', 'true');
    await expect(save).toBeDisabled();
    await helpful.click();
    await expect(panel).toHaveCount(0);
    expect(state.reasons).toEqual([]);
    expect(state.comment).toBe('');
    await down.click();
    await expect(save).toBeEnabled();
    await expect(inaccurate).not.toBeChecked();
    await expect(details).toHaveValue('');
    await down.click();
    await expect(reply.getByRole('status', { name: 'Vote status' })).toHaveText('Vote removed');
    await screenshot('removed');
    expect(boundary.writes).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    expect(boundary.errors).toEqual([]);
  });
}
