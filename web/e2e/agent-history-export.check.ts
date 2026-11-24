import { test, expect } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

for (const status of ['ready', 'stopped', 'failed'] as const) {
  for (const sandboxStatus of ['active', 'paused'] as const) {
    test(`LOCAL: ${status} Agent in ${sandboxStatus} Sandbox exports retained conversation while raw runtime history fails`, async ({
      page,
      baseURL,
    }) => {
      const fixture = new LocalStartup();
      fixture.agent = { ...fixture.agent, status };
      fixture.sandbox = { ...fixture.sandbox, status: sandboxStatus };
      fixture.agent = {
        ...fixture.agent,
        sandbox: { ...fixture.agent.sandbox, status: sandboxStatus },
      };
      const content = '**检查日期：**2026 年\n\nRetained answer with original bytes.';
      const transcript = {
        agent_id: fixture.agent.id,
        through_seq: 8,
        turns: [
          {
            turn_sequence: 2,
            started_at: localTime,
            owner_message: 'Synthetic question',
            agent_messages: [content],
            messages: [{ sequence: 7, role: 'assistant', content, appended_at: localTime }],
            outcome: {
              type: 'failed',
              at: localTime,
              failure: {
                class: 'conversation_output_gap',
                detail: 'Recorded output is incomplete.',
              },
            },
          },
        ],
      };
      await fixture.route(page, baseURL!);
      const toolResult = { text: 'Recorded tool result', truncated: false };
      const events = Array.from({ length: 8 }, (_, offset) => ({
        seq: offset + 1,
        appended_at: localTime,
        kind: { type: 'access_redacted' },
      }));
      await page.route('**/api/v1/agents/agt_contract/events*', (route) => {
        const query = new URL(route.request().url()).searchParams;
        if (query.get('tool_output') !== 'true') return route.fallback();
        return route.fulfill({
          json: {
            items: [
              ...events.slice(0, 3),
              {
                ...events[3]!,
                kind: {
                  type: 'tool_call_started',
                  call_id: 'fixture-tool',
                  title: 'Fixture tool',
                  kind: 'execute',
                  status: 'in_progress',
                },
              },
              {
                ...events[4]!,
                kind: { type: 'tool_output_reported', call_id: 'fixture-tool', output: toolResult },
              },
              ...events.slice(5),
            ].filter((event) => event.seq > Number(query.get('after') ?? 0)),
          },
        });
      });
      let exports = 0;
      await page.route('**/api/v1/agents/agt_contract/transcript*', (route) => {
        exports += 1;
        return route.fulfill({ json: transcript });
      });
      await page.route('**/api/v1/agents/agt_contract/history', (route) =>
        route.fulfill({
          status: 503,
          contentType: 'application/problem+json',
          json: { code: 'history_export_unavailable', detail: 'Synthetic raw dependency failure' },
        }),
      );
      await page.goto('/agents/agt_contract');
      await page.getByRole('button', { name: 'More Agent actions' }).click();
      if (sandboxStatus === 'active') {
        await page.getByRole('menuitem', { name: 'Export raw history', exact: true }).click();
        await expect(
          page.getByText(/Raw history is currently unavailable from the runtime/),
        ).toBeVisible();
      } else {
        await expect(page.getByRole('menuitem', { name: /Export raw history/ })).toHaveAttribute(
          'aria-disabled',
          'true',
        );
        await page.keyboard.press('Escape');
      }
      const before = exports;
      await page.getByRole('button', { name: 'More Agent actions' }).click();
      const downloaded = page.waitForEvent('download');
      await page
        .getByRole('menuitem', { name: 'Export retained conversation', exact: true })
        .click();
      const file = await downloaded;
      expect(file.suggestedFilename()).toBe(
        'cma-agent-agt_contract-retained-conversation-v2.jsonl',
      );
      const stream = await file.createReadStream();
      const chunks: Buffer[] = [];
      for await (const chunk of stream!) chunks.push(Buffer.from(chunk));
      const records = Buffer.concat(chunks)
        .toString('utf8')
        .trimEnd()
        .split('\n')
        .map((line) => JSON.parse(line));
      expect(records[0]).toMatchObject({
        format: 'cma.retained-conversation',
        version: 2,
        through_seq: 8,
        coverage: { raw_history: false, completeness: 'not_certified' },
      });
      expect(records[1]).toMatchObject({
        sequence: 4,
        tool_event: { type: 'tool_call_started', call_id: 'fixture-tool' },
      });
      expect(records[2]).toMatchObject({
        sequence: 5,
        tool_event: { type: 'tool_output_reported', call_id: 'fixture-tool', output: toolResult },
      });
      expect(records[3]).toMatchObject({
        content,
        outcome: { failure: { class: 'conversation_output_gap' } },
      });
      expect(exports).toBeGreaterThan(before);
      expect(fixture.writes).toEqual([]);
      expect(fixture.unexpected).toEqual([]);
      expect(fixture.errors).toEqual([]);
    });
  }
}
