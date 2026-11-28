import { registerAgentRedesignCases } from './redesign-agent.cases';
import { test, expect, type Page } from '@playwright/test';
import { binding, LocalAgentV2 } from './agent-v2.fixture';

function running() {
  const fixture = new LocalAgentV2();
  fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
  fixture.append({ type: 'status_changed', from: 'ready', to: 'working' });
  fixture.append({
    type: 'tool_started',
    call_id: 'synthetic-tool',
    title: 'Inspect synthetic file',
    kind: 'read',
    status: 'running',
  });
  return fixture;
}
function complete(fixture: LocalAgentV2) {
  fixture.append({
    type: 'tool_updated',
    call_id: 'synthetic-tool',
    title: null,
    status: 'completed',
  });
  fixture.append({ type: 'message', role: 'assistant', content: 'Synthetic final answer' });
  fixture.append({ type: 'turn_completed', turn_sequence: 1 });
  fixture.append({ type: 'status_changed', from: 'working', to: 'ready' });
}
function noWrites(fixture: LocalAgentV2) {
  expect(fixture.writes).toEqual([]);
  expect(fixture.boundary.writes).toEqual([]);
  expect(fixture.boundary.unexpected).toEqual([]);
  expect(fixture.boundary.errors).toEqual([]);
  expect(fixture.errors).toEqual([]);
}
const task = (page: Page) =>
  page.getByRole('tree', { name: 'Personal Agents' }).getByRole('treeitem').first();
const headerStatus = (page: Page, label: string) =>
  page.locator('.agent-toolbar__status').getByText(label, { exact: true });

test('LOCAL completed-turn recovery agrees across task, header and tools after reload and reopen', async ({
  page,
  baseURL,
}) => {
  const fixture = running();
  try {
    await fixture.route(page, baseURL!);
    await page.goto('/agents/agt_contract');
    await expect.poll(() => fixture.streams.size).toBe(1);
    await expect(task(page)).toHaveAccessibleName(/Working/);
    await expect(headerStatus(page, 'Working')).toBeVisible();
    const progress = page.locator('.turn-activity');
    await expect(progress.getByText('Running a tool', { exact: true })).toBeVisible();
    await page.getByRole('button', { name: 'View activity', exact: true }).click();
    await expect(page.locator('.thinking__activity')).toContainText(
      'Inspect synthetic file - running',
    );
    fixture.gap(1);
    await expect(page.getByText('Refreshing live updates…', { exact: true })).toHaveCount(1);
    complete(fixture);
    await expect(page.getByText('Synthetic final answer', { exact: true })).toBeVisible();
    await page.locator('.tool-calls__toggle').click();
    await expect(
      page
        .getByRole('article', { name: 'Tool call: Inspect synthetic file' })
        .getByRole('img', { name: 'Completed', exact: true }),
    ).toBeVisible();
    await expect(task(page)).toHaveAccessibleName(/Turn ended/);
    await expect(headerStatus(page, 'Ready')).toBeVisible();
    await expect(progress.getByText('Turn ended', { exact: true })).toBeVisible();
    await expect(page.getByText('Refreshing live updates…', { exact: true })).toHaveCount(0);
    await expect(page.locator('[data-streaming]')).toHaveCount(0);
    await page.reload();
    await expect.poll(() => fixture.streams.size).toBe(1);
    fixture.gap(null);
    await expect(task(page)).toHaveAccessibleName(/Turn ended/);
    await expect(headerStatus(page, 'Ready')).toBeVisible();
    await expect(page.getByText('Refreshing live updates…', { exact: true })).toHaveCount(0);
    await page.goto('/agent-manager');
    await task(page).click();
    await expect(page.getByText('Synthetic final answer', { exact: true })).toBeVisible();
    await expect.poll(() => fixture.streams.size).toBe(1);
    const next = fixture.items.length + 1;
    fixture.append(
      { type: 'turn_started', turn_sequence: next },
      { command_id: 'cmd_next', response_id: 'resp_next', turn_sequence: next },
    );
    fixture.append({ type: 'status_changed', from: 'ready', to: 'working' });
    fixture.gap(next);
    await expect(page.getByText('Refreshing live updates…', { exact: true })).toHaveCount(1);
    fixture.append({ type: 'turn_completed', turn_sequence: 1 });
    await expect(task(page)).toHaveAccessibleName(/Working/);
    await expect(headerStatus(page, 'Working')).toBeVisible();
    await expect(page.getByText('Refreshing live updates…', { exact: true })).toHaveCount(1);
    fixture.append({ type: 'turn_completed', turn_sequence: next });
    fixture.append({ type: 'status_changed', from: 'working', to: 'ready' });
    await expect(page.getByText('Refreshing live updates…', { exact: true })).toHaveCount(0);
    await expect(task(page)).toHaveAccessibleName(/Turn ended/);
    noWrites(fixture);
  } finally {
    await fixture.close(page);
  }
});

test('LOCAL running reload shows loading until activity is known, then agrees with recorded tool state', async ({
  page,
  baseURL,
}) => {
  const fixture = running();
  fixture.holdSnapshot = true;
  fixture.boundary.workspace = {
    ...fixture.boundary.workspace,
    agents: fixture.boundary.workspace.agents.map((agent) => ({
      ...agent,
      task_status: 'completed' as const,
    })),
  };
  try {
    await fixture.route(page, baseURL!);
    await page.goto('/agents/agt_contract');
    await expect.poll(() => fixture.snapshots).toBe(2);
    await expect(page.getByText('Loading conversation activity.', { exact: true })).toBeVisible();
    await expect(task(page)).toHaveAccessibleName(/Loading activity/);
    await expect(headerStatus(page, 'Loading activity')).toBeVisible();
    await expect.poll(() => fixture.snapshotPending).toBe(true);
    fixture.releaseSnapshot();
    await expect(page.getByText('Loading conversation activity.', { exact: true })).toHaveCount(0);
    await expect(task(page)).toHaveAccessibleName(/Working/);
    await expect(headerStatus(page, 'Working')).toBeVisible();
    const activity = page.getByRole('button', { name: 'View activity', exact: true });
    await activity.click();
    await expect(activity).toHaveAttribute('aria-expanded', 'true');
    await expect(page.locator('.thinking__activity')).toContainText(
      'Inspect synthetic file - running',
    );
    fixture.holdSnapshot = true;
    await page.reload();
    await expect.poll(() => fixture.snapshots).toBe(4);
    await expect(page.getByText('Loading conversation activity.', { exact: true })).toBeVisible();
    await expect(task(page)).toHaveAccessibleName(/Loading activity/);
    await expect.poll(() => fixture.snapshotPending).toBe(true);
    fixture.releaseSnapshot();
    await expect(task(page)).toHaveAccessibleName(/Working/);
    await expect(headerStatus(page, 'Working')).toBeVisible();
    noWrites(fixture);
  } finally {
    await fixture.close(page);
  }
});

for (const failure of ['503', 'stream close']) {
  test(`LOCAL idle ${failure} recovery clears the connection error without reviving a completed reply`, async ({
    page,
    baseURL,
  }) => {
    const fixture = running();
    complete(fixture);
    let unavailable = false;
    try {
      await fixture.route(page, baseURL!);
      await page.route('**/api/v2/agents/agt_contract/events?**', (route) =>
        unavailable
          ? route.fulfill({
              status: 503,
              json: {
                error: {
                  type: 'server_error',
                  code: 'agent_read_unavailable',
                  message: 'Synthetic temporary observation failure',
                  param: null,
                },
              },
            })
          : route.fallback(),
      );
      await page.goto('/agents/agt_contract');
      await expect(page.getByText('Synthetic final answer', { exact: true })).toBeVisible();
      await expect.poll(() => fixture.streams.size).toBe(1);
      unavailable = failure === '503';
      fixture.endTails();
      await expect(
        page.getByText('Live updates are reconnecting. Recorded activity is retained.', {
          exact: true,
        }),
      ).toBeVisible();
      await expect(page.getByText('Refreshing live updates…', { exact: true })).toHaveCount(0);
      await expect(page.getByText('Synthetic final answer', { exact: true })).toBeVisible();
      unavailable = false;
      await expect.poll(() => fixture.streams.size).toBe(1);
      fixture.gap(null, 'reconnected');
      await expect(
        page.getByText('Live updates are reconnecting. Recorded activity is retained.', {
          exact: true,
        }),
      ).toHaveCount(0);
      await expect(page.getByText('Synthetic final answer', { exact: true })).toBeVisible();
      await expect(page.getByText('Refreshing live updates…', { exact: true })).toHaveCount(0);
      noWrites(fixture);
    } finally {
      await fixture.close(page);
    }
  });
}

for (const reportedStatus of ['pending', 'blocked']) {
  test(`LOCAL ended turn with ${reportedStatus} plan keeps a scoped check after recovery and later work`, async ({
    page,
    baseURL,
  }) => {
    const fixture = running();
    fixture.append({
      type: 'plan_set',
      entries: [
        { title: 'Write the answer', status: 'completed' },
        { title: 'Check supporting file', status: reportedStatus },
      ],
    });
    complete(fixture);
    const notice =
      reportedStatus === 'pending'
        ? 'Turn ended · Check unfinished plan steps and output.'
        : 'Turn ended · Check plan progress and output.';
    try {
      await fixture.route(page, baseURL!);
      await page.goto('/agents/agt_contract');
      const plans = page.getByRole('region', { name: 'Plan', exact: true });
      const checkEnded = async () => {
        await expect(task(page)).toHaveAccessibleName(/Turn ended/);
        await expect(headerStatus(page, 'Ready')).toBeVisible();
        await expect(plans).toHaveCount(1);
        await expect(plans).toContainText(notice);
        await expect(plans.getByRole('button', { name: 'View plan' })).toHaveAttribute(
          'aria-expanded',
          'false',
        );
        await expect(page.getByText('Synthetic final answer', { exact: true })).toBeVisible();
      };
      await checkEnded();
      await expect.poll(() => fixture.streams.size).toBe(1);
      fixture.endTails();
      await expect(
        page.getByText('Live updates are reconnecting. Recorded activity is retained.', {
          exact: true,
        }),
      ).toBeVisible();
      await expect.poll(() => fixture.streams.size).toBe(1);
      fixture.gap(null, 'reconnected');
      await checkEnded();
      await page.reload();
      await checkEnded();
      await page.goto('/agent-manager');
      await task(page).click();
      await checkEnded();
      await expect.poll(() => fixture.streams.size).toBe(1);
      const next = fixture.items.length + 1;
      fixture.append(
        { type: 'turn_started', turn_sequence: next },
        { command_id: 'cmd_followup', response_id: 'resp_followup', turn_sequence: next },
      );
      fixture.append({ type: 'status_changed', from: 'ready', to: 'working' });
      fixture.append({
        type: 'plan_set',
        entries: [{ title: 'Confirm the follow-up', status: 'completed' }],
      });
      fixture.append({ type: 'message', role: 'assistant', content: 'Follow-up output retained.' });
      fixture.append({ type: 'turn_completed', turn_sequence: next });
      fixture.append({ type: 'status_changed', from: 'working', to: 'ready' });
      await expect(plans).toHaveCount(2);
      await expect(plans.first()).toContainText(notice);
      await expect(plans.last()).toContainText('Turn ended · 1 plan step reported complete');
      await expect(plans.last()).not.toContainText(/Check unfinished|Check plan progress/);
      await expect(task(page)).toHaveAccessibleName(/Turn ended/);
      await expect(headerStatus(page, 'Ready')).toBeVisible();
      await expect(page.getByText('Follow-up output retained.', { exact: true })).toBeVisible();
      await expect(page.getByText('Synthetic final answer', { exact: true })).toBeVisible();
      noWrites(fixture);
    } finally {
      await fixture.close(page);
    }
  });
}

registerAgentRedesignCases();
