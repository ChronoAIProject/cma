import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { binding, cursor, LocalAgentV2 } from './agent-v2.fixture';
import { LocalUsageMetrics } from './usage-metrics.fixture';
import { LocalUsageFilterOptions } from './usage-filter-options.fixture';
import { PerformanceObserver } from './ui-performance-observer';

test('Agent inventory first render and usable rows', async ({ page, baseURL }, info) => {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL!);
  await page.route('**/api/v1/agents/provenance**', (route) =>
    route.fulfill({
      json: {
        items: [
          {
            agent_id: boundary.agent.id,
            title: 'Recorded Agent',
            status: 'ready',
            created_at: boundary.agent.created_at,
            updated_at: boundary.agent.created_at,
            creation_origin: 'manual',
            channel_usage: {
              active: false,
              has_history: false,
              first_used_at: null,
              last_used_at: null,
            },
          },
        ],
      },
    }),
  );
  const observed = new PerformanceObserver(page);
  await observed.delay();
  await page.goto('/agents');
  await page.getByRole('heading', { name: 'Agents', exact: true }).waitFor();
  observed.mark('shell');
  await page.getByRole('link', { name: 'Recorded Agent', exact: true }).waitFor();
  observed.mark('content');
  await page.getByRole('button', { name: 'Creation origin' }).click();
  await page.getByRole('menuitemradio', { name: 'Created manually', exact: true }).waitFor();
  observed.mark('interactive');
  await page.keyboard.press('Escape');
  await page.waitForTimeout(500);
  expect(boundary.unexpected).toEqual([]);
  expect(boundary.errors).toEqual([]);
  await observed.save(info);
});

test('Agent open, send, active stream and cached return', async ({ page, baseURL }, info) => {
  const fixture = new LocalAgentV2();
  const observed = new PerformanceObserver(page);
  let events: ReturnType<typeof setInterval> | undefined;
  try {
    fixture.append(
      { type: 'turn_started', turn_sequence: 1 },
      {
        response_id: 'resp_old',
        command_id: 'cmd_old',
        turn_sequence: 1,
      },
    );
    fixture.append({ type: 'message', role: 'user', content: 'Previous request' });
    fixture.append({ type: 'message', role: 'assistant', content: 'Previous recorded answer' });
    fixture.append({ type: 'turn_completed', turn_sequence: 1 });
    await fixture.route(page, baseURL!);
    await page.route('**/api/v2/ui/task-experience', (route) =>
      route.fulfill({
        status: 404,
        json: { code: 'not_found', detail: 'Optional capability unavailable in this fixture.' },
      }),
    );
    await page.route('**/api/v1/agents/agt_contract/transcript', (route) =>
      route.fulfill({
        json: {
          agent_id: 'agt_contract',
          through_seq: 4,
          turns: [
            {
              turn_sequence: 1,
              started_at: fixture.started,
              owner_message: 'Previous request',
              agent_messages: ['Previous recorded answer'],
              messages: [
                {
                  sequence: 2,
                  role: 'user',
                  content: 'Previous request',
                  appended_at: fixture.started,
                },
                {
                  sequence: 3,
                  role: 'assistant',
                  content: 'Previous recorded answer',
                  appended_at: fixture.started,
                },
              ],
              outcome: { type: 'completed', at: fixture.started },
            },
          ],
        },
      }),
    );
    await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
      route.fulfill({
        json: {
          object: 'cma.agent.event_head',
          schema_version: 1,
          agent_id: 'agt_contract',
          incarnation: 'a'.repeat(64),
          through_cursor: cursor(4),
          active_turn: null,
        },
      }),
    );
    await observed.delay();
    await page.goto('/agents/agt_contract');
    await page.locator('.console__empty, .agent-surface').first().waitFor();
    observed.mark('shell');
    const input = page.getByRole('textbox', { name: 'Message', exact: true });
    await input.waitFor();
    observed.mark('composer');
    await page.getByText('Previous recorded answer', { exact: true }).waitFor();
    observed.mark('content');
    await input.fill('Local performance request');
    await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
    observed.mark('interactive');
    await expect.poll(() => fixture.streams.size).toBe(1);
    observed.mark('historyReady');
    observed.begin('send');
    await page.getByRole('button', { name: 'Send message', exact: true }).click();
    await expect.poll(() => fixture.writes.length, { intervals: [10] }).toBe(1);
    observed.mark('serverAdmission');
    fixture.append({ type: 'turn_started', turn_sequence: 5 }, { ...binding, turn_sequence: 5 });
    fixture.append({ type: 'message', role: 'user', content: 'Local performance request' });
    fixture.live('text_delta', 'First streamed token', 5);
    await page.getByText('First streamed token', { exact: true }).waitFor();
    observed.mark('firstToken');
    await page.waitForTimeout(1000);
    observed.begin('streaming');
    let tokens = 0;
    events = setInterval(
      () =>
        fixture.append({
          type: 'usage',
          input_tokens: 1,
          output_tokens: ++tokens,
          context_used: tokens + 1,
          context_window: 100000,
        }),
      200,
    );
    await page.waitForTimeout(60_000);
    clearInterval(events);
    observed.mark('streamingEnd');
    if (process.env.CMA_REQUEST_BASELINE !== '1') {
      const queueReads = observed.requests.filter(
        (read) => read.phase === 'streaming' && read.path.endsWith('/queued-inputs'),
      );
      expect(queueReads.length).toBeLessThanOrEqual(7);
      expect(
        observed.requests.filter((read) => /^\/api\/v2\/responses\/[^/]+$/.test(read.path)),
      ).toEqual([]);
    }
    fixture.append({ type: 'message', role: 'assistant', content: 'Retained recorded answer' });
    fixture.append({ type: 'turn_completed', turn_sequence: 5 });
    await page.getByText('Retained recorded answer', { exact: true }).waitFor();
    await page.getByRole('link', { name: 'CMA home', exact: true }).click();
    observed.begin('return');
    await page.goBack();
    await page.getByText('Retained recorded answer', { exact: true }).waitFor();
    observed.mark('cachedContent');
    await input.fill('Unsent draft');
    await expect(page.getByRole('button', { name: 'Send message', exact: true })).toBeEnabled();
    observed.mark('cachedInteractive');
    expect(fixture.writes).toHaveLength(1);
    expect(fixture.errors).toEqual([]);
    expect(fixture.boundary.errors).toEqual([]);
    expect(fixture.boundary.unexpected).toEqual([]);
  } catch (error) {
    await info.attach('local-fixture-surface', {
      body: await page.locator('.agent-surface').innerText(),
      contentType: 'text/plain',
    });
    throw error;
  } finally {
    clearInterval(events);
    await observed.save(info);
    await info.attach('fixture-boundary', {
      body: JSON.stringify({
        writes: fixture.writes.length,
        streams: fixture.streams.size,
        reads: fixture.reads,
        errors: fixture.errors,
        scriptErrors: fixture.boundary.errors,
        unexpected: fixture.boundary.unexpected,
      }),
      contentType: 'application/json',
    });
    await fixture.close(page);
  }
});

for (const flow of ['operations', 'usage']) {
  test(`${flow} shell, content and filters`, async ({ page, baseURL }, info) => {
    const boundary = new LocalStartup();
    const metrics = new LocalUsageMetrics();
    await boundary.route(page, baseURL!);
    await metrics.session(page, ['use_own_sandboxes', 'read_all_sandboxes']);
    await metrics.route(page);
    await new LocalUsageFilterOptions().route(page);
    const observed = new PerformanceObserver(page);
    await observed.delay();
    await page.goto(flow === 'operations' ? '/operations?tab=sandboxes' : '/usage');
    const surface =
      flow === 'operations'
        ? page.getByRole('region', { name: 'Sandbox resource dashboard' })
        : page.getByRole('dialog', { name: 'Your usage' });
    await surface.waitFor();
    observed.mark('shell');
    await surface.getByRole('button', { name: 'Research Sandbox', exact: true }).waitFor();
    observed.mark('content');
    await surface.getByRole('button', { name: 'Usage time range' }).click();
    await page.getByRole('menu').waitFor();
    observed.mark('interactive');
    await page.keyboard.press('Escape');
    await page.waitForTimeout(500);
    expect(metrics.creates).toHaveLength(1);
    expect(boundary.errors).toEqual([]);
    expect(boundary.unexpected).toEqual([]);
    await observed.save(info);
  });
}
