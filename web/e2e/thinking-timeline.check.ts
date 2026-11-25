import { createServer, type ServerResponse } from 'node:http';
import { test, expect } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

for (const theme of ['light', 'dark']) {
  test(`LOCAL ${theme} Thinking follows the reply, restores its clock and separates finished tools`, async ({
    page,
    baseURL,
  }, info) => {
    const boundary = new LocalStartup();
    const started = new Date(Date.now() - 120_000).toISOString();
    let completed = false;
    let stream: ServerResponse | undefined;
    const fixture = createServer((request, response) => {
      response.writeHead(200, {
        'Content-Type': 'text/event-stream',
        'Access-Control-Allow-Origin': baseURL!,
        'Access-Control-Allow-Credentials': 'true',
        'Access-Control-Allow-Headers': 'content-type, x-request-id, last-event-id',
        'Access-Control-Allow-Methods': 'GET, OPTIONS',
      });
      if (request.method === 'OPTIONS') return response.end();
      response.write(': connected\n\n');
      stream = response;
    });
    await new Promise<void>((resolve) => fixture.listen(0, '127.0.0.1', resolve));
    const address = fixture.address();
    if (address === null || typeof address === 'string') throw new Error('Missing fixture address');
    const fixtureURL = `http://127.0.0.1:${address.port}/events`;
    const emit = (type: string, fields: Record<string, unknown>, seq?: number) => {
      if (stream === undefined || stream.destroyed) throw new Error('Live stream is not connected');
      stream.write(
        `${seq === undefined ? '' : `id: ${seq}\n`}data: ${JSON.stringify({
          type,
          threadId: 'agt_contract',
          runId: '1',
          timestamp: started,
          ...(seq === undefined ? { 'cma:live': true } : {}),
          ...fields,
        })}\n\n`,
      );
    };
    boundary.events = [
      { seq: 1, appended_at: started, kind: { type: 'turn_started', turn_sequence: 1 } },
      {
        seq: 2,
        appended_at: started,
        kind: { type: 'owner_message', content: 'Inspect the repository.' },
      },
      {
        seq: 3,
        appended_at: started,
        kind: {
          type: 'tool_call_started',
          call_id: 'slow',
          title: 'Run tests',
          kind: 'execute',
          status: 'in_progress',
        },
      },
      {
        seq: 4,
        appended_at: started,
        kind: {
          type: 'tool_call_started',
          call_id: 'fast',
          title: 'Read package',
          kind: 'read',
          status: 'in_progress',
        },
      },
    ];
    const append = (kind: Record<string, unknown>) => {
      const seq = (boundary.events.at(-1)?.seq ?? 0) + 1;
      boundary.events.push({ seq, appended_at: started, kind });
      return seq;
    };
    try {
      await boundary.route(page, baseURL!);
      await page.route(fixtureURL, (route) => route.continue());
      await page.route('**/auth/connections/check', (route) =>
        route.fulfill({
          json: {
            owner: 'local-owner',
            ready: true,
            checked_at: started,
            services: [],
          },
        }),
      );
      await page.route('**/api/v1/agents/agt_contract/resources', (route) =>
        route.fulfill({
          json: {
            agent_id: 'agt_contract',
            sandbox_id: 'sbx_route',
            uptime_ms: 120000,
            cpu_usage_usec: 100000,
            cpu_millis_limit: 4000,
            memory_bytes_used: 1048576,
            memory_bytes_limit: 8388608,
            memory_bytes_peak: null,
            stall_permille: null,
            ceiling_hits: null,
            allocation_failures: null,
            kills: null,
            received_bytes: 100,
            sent_bytes: 50,
          },
        }),
      );
      await page.route('**/api/v1/agui/events*', (route) => route.continue({ url: fixtureURL }));
      await page.route('**/api/v1/agents/agt_contract/source', (route) =>
        route.fulfill({
          json: {
            agent_id: 'agt_contract',
            origin: { type: 'manual' },
          },
        }),
      );
      await page.route('**/api/v1/agents/agt_contract/transcript', (route) =>
        route.fulfill({
          json: {
            agent_id: 'agt_contract',
            through_seq: boundary.events.at(-1)!.seq,
            turns: [
              {
                turn_sequence: 1,
                started_at: started,
                owner_message: 'Inspect the repository.',
                agent_messages: [],
                messages: boundary.events
                  .filter((event) =>
                    ['owner_message', 'agent_message'].includes(String(event.kind.type)),
                  )
                  .map((event) => ({
                    sequence: event.seq,
                    role: event.kind.type === 'owner_message' ? 'user' : 'assistant',
                    content: event.kind.content,
                    appended_at: event.appended_at,
                  })),
                outcome: { type: completed ? 'completed' : 'active' },
              },
            ],
          },
        }),
      );
      await page.goto('/agents/agt_contract');
      const toggle = page.getByRole('button', { name: 'View activity', exact: true });
      const elapsed = async () => {
        const text = await page.locator('.thinking__elapsed').innerText();
        expect(text).toMatch(/^\d+[dhms](?: \d+[hms])?$/);
        const seconds = { d: 86400, h: 3600, m: 60, s: 1 };
        return [...text.matchAll(/(\d+)([dhms])/g)].reduce(
          (total, match) => total + Number(match[1]) * seconds[match[2] as keyof typeof seconds],
          0,
        );
      };
      await expect(toggle).toBeVisible();
      expect(boundary.errors).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      await expect.poll(() => stream !== undefined && !stream.destroyed).toBe(true);
      expect(await elapsed()).toBeGreaterThanOrEqual(120);
      await toggle.focus();
      await page.keyboard.press('Enter');
      await expect(toggle).toBeFocused();
      await expect(toggle).toHaveAttribute('aria-expanded', 'true');
      await expect(page.locator('.thinking__activity')).toHaveCount(2);
      await expect(page.locator('.agent-event--tool')).toHaveCount(0);
      emit('REASONING_MESSAGE_CONTENT', { messageId: 'reason', delta: 'Inspecting dependencies' });
      await expect(page.locator('.thinking__reasoning')).toHaveText('Inspecting dependencies');
      const text = 'Live progress remains readable.\n\n'.repeat(70);
      const messageSeq = append({ type: 'agent_message', content: text });
      emit('TEXT_MESSAGE_START', { messageId: 'update', role: 'assistant' });
      emit('TEXT_MESSAGE_CONTENT', { messageId: 'update', delta: text });
      emit('TEXT_MESSAGE_END', { messageId: 'update' }, messageSeq);
      await expect(page.locator('.message--assistant')).toContainText(
        'Live progress remains readable.',
      );
      const scroller = page.locator('.agent-surface__conversation');
      const fits = () =>
        page.evaluate(() => {
          const dock = document.querySelector('.thinking')!.getBoundingClientRect();
          const history = document
            .querySelector('.agent-surface__conversation')!
            .getBoundingClientRect();
          const composer = document.querySelector('.agent-composer')!.getBoundingClientRect();
          return (
            document
              .querySelector('.message-stream')!
              .contains(document.querySelector('.thinking')) &&
            dock.top >= history.top - 1 &&
            dock.bottom <= history.bottom + 1 &&
            history.bottom <= composer.top + 1 &&
            dock.left >= 0 &&
            dock.right <= innerWidth &&
            document.documentElement.scrollWidth <= innerWidth + 1
          );
        });
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        theme === 'dark',
      );
      for (const width of [1440, 390, 320]) {
        await page.setViewportSize({ width, height: 900 });
        await toggle.scrollIntoViewIfNeeded();
        await expect.poll(fits).toBe(true);
        await scroller.evaluate((element) => {
          element.scrollTop = 0;
          element.dispatchEvent(new Event('scroll'));
        });
        await expect
          .poll(() =>
            page.locator('.thinking').evaluate((element) => {
              const history = element.closest('.agent-surface__conversation')!;
              return element.getBoundingClientRect().top >= history.getBoundingClientRect().bottom;
            }),
          )
          .toBe(true);
        await toggle.scrollIntoViewIfNeeded();
        await expect.poll(fits).toBe(true);
        await page.screenshot({
          path: info.outputPath(`thinking-${theme}-${width}.png`),
          animations: 'disabled',
        });
      }
      await page.emulateMedia({ reducedMotion: 'reduce' });
      await expect(toggle).toBeVisible();
      await expect.poll(fits).toBe(true);
      await page.setViewportSize({ width: 1440, height: 900 });
      emit(
        'TOOL_CALL_END',
        { toolCallId: 'fast', 'cma:status': 'completed' },
        append({ type: 'tool_call_updated', call_id: 'fast', status: 'completed' }),
      );
      await expect(page.locator('.thinking__activity')).toHaveCount(1);
      await page.locator('.tool-calls__toggle[aria-expanded="false"]').click();
      await expect(page.locator('.tool-call__literal')).toHaveText(['Read package']);
      emit(
        'TOOL_CALL_END',
        { toolCallId: 'slow', 'cma:status': 'failed' },
        append({ type: 'tool_call_updated', call_id: 'slow', status: 'failed' }),
      );
      await expect(page.locator('.thinking__activity')).toHaveCount(0);
      await expect(page.locator('.tool-call__literal')).toHaveText(['Read package', 'Run tests']);
      const beforeNavigation = await elapsed();
      await page.locator('.sidebar').getByRole('link', { name: 'CMA home', exact: true }).click();
      await expect(toggle).toHaveCount(0);
      stream = undefined;
      await page.goBack();
      await expect(toggle).toBeVisible();
      expect(await elapsed()).toBeGreaterThanOrEqual(beforeNavigation);
      await page.locator('.tool-calls__toggle[aria-expanded="false"]').click();
      await expect(page.locator('.tool-call__literal')).toHaveText(['Read package', 'Run tests']);
      await page.reload();
      await expect(toggle).toBeVisible();
      expect(await elapsed()).toBeGreaterThanOrEqual(beforeNavigation);
      await page.locator('.tool-calls__toggle[aria-expanded="false"]').click();
      await expect(page.locator('.tool-call__literal')).toHaveText(['Read package', 'Run tests']);
      await expect.poll(() => stream !== undefined && !stream.destroyed).toBe(true);
      completed = true;
      emit(
        'RUN_FINISHED',
        { outcome: { type: 'complete' } },
        append({ type: 'turn_completed', turn_sequence: 1 }),
      );
      await expect(toggle).toHaveCount(0);
      await expect(page.locator('.agent-event--tool')).toHaveCount(2);
      expect(boundary.writes).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.errors).toEqual([]);
    } finally {
      await page.close();
      stream?.destroy();
      fixture.closeAllConnections();
      await new Promise<void>((resolve) => fixture.close(() => resolve()));
    }
  });
}
