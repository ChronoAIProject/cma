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
const header = (page: Page) =>
  page.getByRole('button', { name: 'Show Agent details', exact: true });

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
    await expect(header(page)).toHaveAttribute('title', /Working/);
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
    await expect(task(page)).toHaveAccessibleName(/Completed/);
    await expect(header(page)).toHaveAttribute('title', /Ready/);
    await expect(page.getByText('Refreshing live updates…', { exact: true })).toHaveCount(0);
    await expect(page.locator('[data-streaming]')).toHaveCount(0);
    await page.reload();
    await expect.poll(() => fixture.streams.size).toBe(1);
    fixture.gap(null);
    await expect(task(page)).toHaveAccessibleName(/Completed/);
    await expect(header(page)).toHaveAttribute('title', /Ready/);
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
    await expect(header(page)).toHaveAttribute('title', /Working/);
    await expect(page.getByText('Refreshing live updates…', { exact: true })).toHaveCount(1);
    fixture.append({ type: 'turn_completed', turn_sequence: next });
    fixture.append({ type: 'status_changed', from: 'working', to: 'ready' });
    await expect(page.getByText('Refreshing live updates…', { exact: true })).toHaveCount(0);
    await expect(task(page)).toHaveAccessibleName(/Completed/);
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
    await expect(page.getByText('Loading conversation activity.', { exact: true })).toBeVisible();
    await expect(task(page)).toHaveAccessibleName(/Loading activity/);
    await expect(header(page)).toHaveAttribute('title', /Loading activity/);
    fixture.releaseSnapshot();
    await page.getByRole('button', { name: 'View activity', exact: true }).click();
    await expect(page.locator('.thinking__activity')).toContainText(
      'Inspect synthetic file - running',
    );
    await expect(task(page)).toHaveAccessibleName(/Working/);
    await expect(header(page)).toHaveAttribute('title', /Working/);
    fixture.holdSnapshot = true;
    await page.reload();
    await expect(page.getByText('Loading conversation activity.', { exact: true })).toBeVisible();
    await expect(task(page)).toHaveAccessibleName(/Loading activity/);
    fixture.releaseSnapshot();
    await expect(task(page)).toHaveAccessibleName(/Working/);
    await expect(header(page)).toHaveAttribute('title', /Working/);
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
