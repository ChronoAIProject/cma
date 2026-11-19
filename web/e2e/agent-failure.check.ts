import { test, expect } from '@playwright/test';
import { LocalStartup, localTime } from './agent-startup-fixtures';

const sentinel = 'PRIVATE_UPSTREAM_SENTINEL';
const started = localTime;
const message = (seq: number, type: string, content: string) => ({
  seq,
  appended_at: started,
  kind: { type, content },
});
const event = (seq: number, kind: Record<string, unknown>) => ({
  seq,
  appended_at: started,
  kind,
});

for (const path of ['live', 'durable', 'transcript'] as const) {
  for (const normalized of [false, true]) {
    test(`LOCAL ${path} ${normalized ? 'normalized timeout' : 'legacy HTML'} failure stays safe after refresh and reload`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalStartup();
      const failure = {
        class: normalized ? 'upstream_timeout' : `provider_failure_${sentinel}`,
        detail: `unexpected status ${normalized ? 524 : 503}: <!doctype html><html><style>.proxy{color:red}</style><body>${sentinel} http://127.0.0.1:9876/private Ray ID: synthetic-ray X-Proxy: synthetic-header</body></html>`,
      };
      const history = [
        event(1, { type: 'turn_started', turn_sequence: 1 }),
        message(2, 'owner_message', 'Earlier question'),
        message(3, 'agent_message', 'Earlier answer'),
        event(4, { type: 'turn_completed', turn_sequence: 1 }),
        event(5, { type: 'turn_started', turn_sequence: 5 }),
        message(6, 'owner_message', 'Current question'),
        message(7, 'agent_message', 'Partial answer'),
      ];
      const terminal = event(8, { type: 'turn_failed', turn_sequence: 5, failure });
      let persisted = path !== 'live';
      let transcriptReads = 0;
      let liveReads = 0;
      fixture.events = path === 'transcript' ? [] : [...history, ...(persisted ? [terminal] : [])];
      await fixture.route(page, baseURL!);
      await page.route('**/api/v1/agents/agt_contract/transcript', (route) => {
        transcriptReads++;
        const turn = (sequence: number, from: number, to: number, outcome: object) => ({
          turn_sequence: sequence,
          started_at: started,
          owner_message: history[from]!.kind.content,
          agent_messages: [history[to]!.kind.content],
          messages: history.slice(from, to + 1).map((item) => ({
            sequence: item.seq,
            role: item.kind.type === 'owner_message' ? 'user' : 'assistant',
            content: item.kind.content,
            appended_at: started,
          })),
          outcome,
        });
        return route.fulfill({
          json: {
            agent_id: fixture.agent.id,
            through_seq: persisted && path !== 'durable' ? 8 : 7,
            turns: [
              turn(1, 1, 2, { type: 'completed', at: started }),
              turn(
                5,
                5,
                6,
                persisted && path !== 'durable'
                  ? { type: 'failed', failure, at: started }
                  : { type: 'active' },
              ),
            ],
          },
        });
      });
      await page.route('**/api/v1/agui/events*', (route) => {
        liveReads++;
        return route.fulfill({
          contentType: 'text/event-stream',
          body:
            path === 'live' && !persisted
              ? `id: 8\ndata: ${JSON.stringify({ type: 'RUN_ERROR', threadId: fixture.agent.id, runId: '5', timestamp: started, message: failure.detail, code: failure.class })}\n\n`
              : ': no new frames\n\n',
        });
      });
      await page.goto('/agents/agt_contract');
      const summary = normalized
        ? 'The Agent run failed because the upstream request timed out (524). Task effects may be incomplete. Review the conversation before sending another message.'
        : 'The Agent run failed. Its delivery or task effects may be incomplete. Review the conversation before sending another message.';
      const check = async () => {
        await expect(page.getByText('Agent run failed', { exact: true })).toBeVisible();
        await expect(page.getByText(summary, { exact: true })).toBeVisible();
        await expect(
          page.getByText(/A new message continues this Agent conversation/),
        ).toBeVisible();
        const details = page.locator('.message-stream__recovery details');
        if (normalized) {
          await expect(details).not.toHaveAttribute('open');
          await details.getByText('Technical details', { exact: true }).click();
          await expect(details.getByText('HTTP status: 524', { exact: true })).toBeVisible();
          await expect(
            details.getByText('Category: upstream_timeout', { exact: true }),
          ).toBeVisible();
        } else {
          await expect(details).toHaveCount(0);
        }
        const dom = await page.locator('body').innerHTML();
        for (const privateText of [
          sentinel,
          '127.0.0.1:9876',
          'synthetic-ray',
          'X-Proxy',
          '.proxy',
          '&lt;!doctype',
        ])
          expect(dom).not.toContain(privateText);
        const text = await page.locator('.message-stream').innerText();
        expect(text.indexOf('Earlier answer')).toBeLessThan(text.indexOf('Current question'));
        expect(text.indexOf('Partial answer')).toBeLessThan(text.indexOf('Agent run failed'));
        await expect(page.getByRole('button', { name: /^Thinking/ })).toHaveCount(0);
        expect(fixture.writes).toEqual([]);
        expect(fixture.unexpected).toEqual([]);
        expect(fixture.errors).toEqual([]);
      };
      await check();
      if (path === 'live') expect(liveReads).toBeGreaterThan(0);
      persisted = true;
      if (path !== 'transcript') fixture.events = [...history, terminal];
      const reads = transcriptReads;
      await page.evaluate(() => {
        window.dispatchEvent(new Event('focus'));
      });
      await expect.poll(() => transcriptReads).toBeGreaterThan(reads);
      expect(fixture.writes).toEqual([]);
      await page.reload();
      await check();
      await page.screenshot({ path: info.outputPath('safe-failure.png'), fullPage: true });
    });
  }
}
