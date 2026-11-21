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
        'cma-agent-agt_contract-retained-conversation-v1.jsonl',
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
        version: 1,
        through_seq: 8,
        coverage: { raw_history: false, completeness: 'not_certified' },
      });
      expect(records[1]).toMatchObject({
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
