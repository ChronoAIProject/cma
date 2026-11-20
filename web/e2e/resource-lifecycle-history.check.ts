import { test, expect } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';
import { noEffects } from './resource-lifecycle.fixture';

for (const width of [1440, 390]) {
  test(`LOCAL retained history stays readable while lifecycle gates mutations at ${width}px`, async ({
    page,
    baseURL,
  }, info) => {
    await page.setViewportSize({ width, height: 900 });
    const boundary = new LocalStartup();
    boundary.sandbox = {
      ...boundary.sandbox,
      status: 'paused',
      lifecycle: {
        pause: { allowed: false, reason: 'not_active' },
        revive: { allowed: false, reason: 'work_in_progress' },
      },
    };
    boundary.events = [
      { seq: 1, appended_at: localTime, kind: { type: 'turn_started', turn_sequence: 1 } },
      {
        seq: 2,
        appended_at: localTime,
        kind: { type: 'owner_message', content: 'Earlier question' },
      },
      {
        seq: 3,
        appended_at: localTime,
        kind: { type: 'agent_message', content: 'Earlier answer' },
      },
      { seq: 4, appended_at: localTime, kind: { type: 'turn_completed', turn_sequence: 1 } },
    ];
    await boundary.route(page, baseURL!);
    await page.route('**/api/v1/agents/agt_contract/transcript', (route) => {
      expect(route.request().method()).toBe('GET');
      return route.fulfill({
        json: {
          agent_id: boundary.agent.id,
          through_seq: 4,
          turns: [
            {
              turn_sequence: 1,
              started_at: localTime,
              owner_message: 'Earlier question',
              agent_messages: ['Earlier answer'],
              messages: [
                { sequence: 2, role: 'user', content: 'Earlier question', appended_at: localTime },
                {
                  sequence: 3,
                  role: 'assistant',
                  content: 'Earlier answer',
                  appended_at: localTime,
                },
              ],
              outcome: { type: 'completed', at: localTime },
            },
          ],
        },
      });
    });
    for (const status of ['paused', 'reviving'] as const) {
      if (status === 'reviving')
        boundary.sandbox = {
          ...boundary.sandbox,
          status,
          lifecycle: {
            pause: { allowed: false, reason: 'not_active' },
            revive: { allowed: false, reason: 'not_paused' },
            operation: { purpose: 'revive', phase: 'home_restore', started_at: localTime },
          },
        };
      boundary.agent = { ...boundary.agent, sandbox: { ...boundary.agent.sandbox, status } };
      await page.goto(`/sandboxes/${boundary.sandbox.id}`);
      await expect(page.getByRole('button', { name: 'Resume', exact: true })).toBeDisabled();
      await page.getByRole('tab', { name: 'Workspaces', exact: true }).click();
      await expect(page.getByRole('button', { name: 'New workspace', exact: true })).toBeDisabled();
      await expect(
        page.getByRole('button', { name: 'New agent in project', exact: true }),
      ).toBeDisabled();
      const retained = page
        .getByRole('region', { name: 'Workspaces', exact: true })
        .getByRole('link')
        .and(page.locator(`a[href="/agents/${boundary.agent.id}"]`));
      await expect(retained).toBeVisible();
      await retained.click();
      await expect(page).toHaveURL(`${baseURL}/agents/${boundary.agent.id}`);
      await expect(page.getByText('Earlier question', { exact: true })).toBeVisible();
      await expect(page.getByText('Earlier answer', { exact: true })).toBeVisible();
      const draft = page.getByRole('textbox', { name: 'Message', exact: true });
      await draft.fill('Keep this draft until ready');
      await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeDisabled();
      await draft.press('Enter');
      await expect(draft).toHaveValue('Keep this draft until ready');
      await page.screenshot({ path: info.outputPath(`${status}-${width}.png`), fullPage: true });
      noEffects(boundary);
    }
    boundary.sandbox = {
      ...boundary.sandbox,
      status: 'active',
      lifecycle: { pause: { allowed: true }, revive: { allowed: false, reason: 'not_paused' } },
    };
    boundary.agent = {
      ...boundary.agent,
      sandbox: { ...boundary.agent.sandbox, status: 'active' },
    };
    const send = page.getByRole('button', { name: 'Send message', exact: true });
    await expect(send).toBeEnabled({ timeout: 10000 });
    await send.click({ trial: true });
    await expect(page.getByRole('textbox', { name: 'Message', exact: true })).toHaveValue(
      'Keep this draft until ready',
    );
    await expect(page.getByText('Earlier answer', { exact: true })).toBeVisible();
    noEffects(boundary);
    await page.goto(`/sandboxes/${boundary.sandbox.id}?tab=workspaces`);
    await expect(page.getByRole('button', { name: 'New workspace', exact: true })).toBeEnabled();
    await expect(
      page.getByRole('button', { name: 'New agent in project', exact: true }),
    ).toBeEnabled();
    noEffects(boundary);
  });
}
