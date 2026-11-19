import { test, expect } from '@playwright/test';
import { cursor, LocalAgentV2 } from './agent-v2.fixture';

test('LOCAL output-only updates retain command scrollback and explicit missing or truncated evidence', async ({
  page,
  baseURL,
}) => {
  const fixture = new LocalAgentV2();
  try {
    await page.setViewportSize({ width: 1600, height: 1000 });
    fixture.append({ type: 'turn_started', turn_sequence: 1 });
    fixture.append({
      type: 'tool_started',
      call_id: 'partial',
      title: 'printf partial',
      kind: 'execute',
      status: 'in_progress',
    });
    fixture.append({
      type: 'tool_output_reported',
      call_id: 'partial',
      output: { text: 'reported prefix', truncated: true },
    });
    fixture.append({
      type: 'tool_started',
      call_id: 'failed',
      title: 'failed command',
      kind: 'execute',
      status: 'failed',
    });
    fixture.append({
      type: 'tool_started',
      call_id: 'cancelled',
      title: 'cancelled command',
      kind: 'execute',
      status: 'cancelled',
    });
    await fixture.route(page, baseURL!);
    await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
      route.fulfill({
        json: {
          object: 'cma.agent.event_head',
          schema_version: 1,
          agent_id: 'agt_contract',
          incarnation: 'a'.repeat(64),
          through_cursor: cursor(fixture.items.length),
          active_turn: null,
        },
      }),
    );
    await page.route('**/api/v1/agents/agt_contract/files/tree**', (route) =>
      route.fulfill({ json: { path: '', entries: [] } }),
    );
    await page.goto('/agents/agt_contract');
    const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
    await computer.getByRole('tab', { name: 'Terminal' }).click();
    const partial = computer.getByRole('article', {
      name: 'Operation: printf partial',
      exact: true,
    });
    await expect(computer.getByLabel('Terminal cursor')).toBeVisible();
    await expect(partial.locator('pre')).toHaveText('reported prefix');
    await expect(partial.getByText('Output truncated', { exact: true })).toBeVisible();
    const failed = computer.getByRole('article', {
      name: 'Operation: failed command',
      exact: true,
    });
    await expect(failed.getByLabel('Recorded command')).toHaveText('$ failed command');
    await expect(failed.getByText('Output not reported', { exact: true })).toBeVisible();
    await expect(
      computer
        .getByRole('article', { name: 'Operation: cancelled command', exact: true })
        .getByText('Output not reported', { exact: true }),
    ).toBeVisible();
    fixture.append({ type: 'turn_cancelled', turn_sequence: 1 });
    await expect(computer.getByLabel('Terminal cursor')).toHaveCount(0);
    await expect(computer.getByText('— agent paused —', { exact: true })).toBeVisible();
    await expect(partial.locator('pre')).toHaveText('reported prefix');
    expect(fixture.writes).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});

test('LOCAL ten thousand operation turns keep complete scrollback while assistant text streams', async ({
  page,
  baseURL,
}, info) => {
  test.setTimeout(120_000);
  const fixture = new LocalAgentV2();
  try {
    await page.setViewportSize({ width: 1600, height: 1000 });
    for (let turn = 1; turn <= 10_000; turn += 1) {
      const sequence = fixture.items.length + 1;
      fixture.append({ type: 'turn_started', turn_sequence: sequence });
      fixture.append({
        type: 'tool_started',
        call_id: `proof-${turn}`,
        title: `printf proof-${turn}`,
        kind: 'execute',
        status: 'in_progress',
      });
      fixture.append({
        type: 'tool_updated',
        call_id: `proof-${turn}`,
        status: 'completed',
        title: null,
        output: { text: `proof-${turn}`, truncated: false },
      });
      fixture.append({ type: 'turn_completed', turn_sequence: sequence });
    }
    fixture.append({ type: 'turn_started', turn_sequence: 40_001 });
    await fixture.route(page, baseURL!);
    await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
      route.fulfill({
        json: {
          object: 'cma.agent.event_head',
          schema_version: 1,
          agent_id: 'agt_contract',
          incarnation: 'a'.repeat(64),
          through_cursor: cursor(fixture.items.length),
          active_turn: null,
        },
      }),
    );
    await page.route('**/api/v2/agents/agt_contract/events/snapshot**', (route) => {
      const url = new URL(route.request().url());
      const offset = Number(url.searchParams.get('page_cursor')?.split('.').at(-1) ?? 0);
      const limit = Number(url.searchParams.get('limit') ?? 100);
      const end = Math.min(fixture.items.length, offset + limit);
      return route.fulfill({
        json: {
          object: 'cma.agent.event_page',
          schema_version: 1,
          agent_id: 'agt_contract',
          incarnation: 'a'.repeat(64),
          through_cursor: cursor(fixture.items.length),
          items: fixture.items.slice(offset, end),
          next_page_cursor:
            end < fixture.items.length
              ? `ags1.${'a'.repeat(64)}.${fixture.items.length}.${end}`
              : null,
        },
      });
    });
    await page.route('**/api/v1/agents/agt_contract/files/tree**', (route) =>
      route.fulfill({ json: { path: '', entries: [] } }),
    );
    await page.goto('/agents/agt_contract');
    const computer = page.getByRole('complementary', { name: 'Agent computer', exact: true });
    await computer.getByRole('tab', { name: 'Terminal' }).click();
    await expect(
      computer.getByRole('article', { name: 'Operation: printf proof-10000', exact: true }),
    ).toBeVisible({ timeout: 90_000 });
    await expect(computer.locator('article')).toHaveCount(10_000);
    await expect(
      computer.getByRole('article', { name: 'Operation: printf proof-1', exact: true }),
    ).toBeVisible();
    const output = await computer.getByText('proof-10000', { exact: true }).elementHandle();
    await page.evaluate(() => {
      const output = document.querySelector('#computer-page-activity')!;
      const observed = {
        changes: 0,
        observer: new MutationObserver((changes) => {
          observed.changes += changes.length;
        }),
      };
      observed.observer.observe(output, { subtree: true, childList: true, characterData: true });
      Object.assign(window, { computerOutputObservation: observed });
    });
    for (let chunk = 0; chunk < 100; chunk += 1) {
      for (const stream of fixture.streams)
        stream.write(
          `event: cma.agent.live\ndata: ${JSON.stringify({ agent_id: 'agt_contract', incarnation: 'a'.repeat(64), turn_sequence: 40_001, after_sequence: fixture.items.length, message_id: 'proof-stream', kind: 'text_delta', delta: 'stream-proof ' })}\n\n`,
        );
    }
    await expect(page.locator('.agent-surface__conversation')).toContainText('stream-proof');
    expect(
      await computer
        .getByText('proof-10000', { exact: true })
        .evaluate((element, previous) => element === previous, output),
    ).toBe(true);
    expect(
      await page.evaluate(
        () =>
          (window as unknown as { computerOutputObservation: { changes: number } })
            .computerOutputObservation.changes,
      ),
    ).toBe(0);
    await computer.getByRole('tab', { name: 'Files', exact: true }).click();
    await expect(page.locator('#computer-page-activity')).toBeHidden();
    await computer.getByRole('tab', { name: 'Terminal', exact: true }).click();
    await expect(computer.locator('article')).toHaveCount(10_000);
    await page.screenshot({ path: info.outputPath('computer-10000-turns.png') });
    expect(fixture.writes).toEqual([]);
    expect(fixture.errors).toEqual([]);
  } finally {
    await fixture.close(page);
  }
});
