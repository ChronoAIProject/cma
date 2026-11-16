import { test, expect } from '@playwright/test';
import { binding, cursor, LocalAgentV2 } from './agent-v2.fixture';
import { commandStatus } from './agent-v2-command.fixture';
import { desktopBoundary } from './desktop-rfb.fixture';

const baseline = process.env.CMA_CONTROL_BASELINE === '1';
for (const pages of [1, 3, 10])
  for (const active of [true, false]) {
    test(`${pages} history pages, ${active ? 'active' : 'idle'}: controls use the live baseline`, async ({
      page,
      baseURL,
    }, info) => {
      const fixture = new LocalAgentV2();
      fixture.boundary.agent = { ...fixture.boundary.agent, desktop: true };
      fixture.append({ type: 'turn_started', turn_sequence: 1 }, binding);
      fixture.append({ type: 'message', role: 'user', content: 'Recorded request' });
      for (let i = 2; i < pages * 3 - 1; i++)
        fixture.append({
          type: 'usage',
          input_tokens: 1,
          output_tokens: 1,
          context_used: 2,
          context_window: 1000,
        });
      fixture.append(
        active
          ? { type: 'message', role: 'assistant', content: 'Recorded preview' }
          : { type: 'turn_completed', turn_sequence: 1 },
      );
      const snapshot = [...fixture.items];
      const through = snapshot.length;
      const marks: Record<string, number> = {};
      const requests: { start: number; end?: number }[] = [];
      let queued = true;
      const queueWrites: unknown[] = [];
      let started = 0;
      const elapsed = () => performance.now() - started;
      let heartbeat: ReturnType<typeof setInterval> | undefined;
      try {
        await fixture.route(page, baseURL!);
        await desktopBoundary(page);
        heartbeat = setInterval(() => {
          for (const stream of fixture.streams) stream.write(': keepalive\n\n');
        }, 100);
        await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
          route.fulfill({
            json: {
              object: 'cma.agent.event_head',
              schema_version: 1,
              agent_id: 'agt_contract',
              incarnation: 'a'.repeat(64),
              through_cursor: cursor(through),
              active_turn: active ? snapshot[0] : null,
            },
          }),
        );
        await page.route('**/api/v1/agents/agt_contract/transcript', (route) =>
          route.fulfill({
            json: {
              agent_id: 'agt_contract',
              through_seq: through,
              turns: [
                {
                  turn_sequence: 1,
                  started_at: fixture.started,
                  owner_message: 'Recorded request',
                  agent_messages: [],
                  messages: [
                    {
                      sequence: 2,
                      role: 'user',
                      content: 'Recorded request',
                      appended_at: fixture.started,
                    },
                  ],
                  outcome: active ? { type: 'active' } : { type: 'completed', at: fixture.started },
                },
              ],
            },
          }),
        );
        await page.route('**/api/v2/agents/agt_contract/events/snapshot**', async (route) => {
          const after = Number(
            new URL(route.request().url()).searchParams.get('page_cursor')?.split('.').at(-1) ?? 0,
          );
          const entry = { start: elapsed(), end: undefined as number | undefined };
          requests.push(entry);
          await new Promise((resolve) => setTimeout(resolve, 5000));
          const next = after + 3;
          await route.fulfill({
            json: {
              object: 'cma.agent.event_page',
              schema_version: 1,
              agent_id: 'agt_contract',
              incarnation: 'a'.repeat(64),
              through_cursor: cursor(through),
              items: snapshot.slice(after, next),
              next_page_cursor: next < through ? `ags1.${'a'.repeat(64)}.${through}.${next}` : null,
            },
          });
          entry.end = elapsed();
        });
        const response = {
          ...commandStatus(fixture.started),
          status: 'queued',
          turn_sequence: null,
          observed_cursor: null,
          delivery: null,
          receipt: {
            ...commandStatus(fixture.started).receipt,
            response_id: 'resp_queue',
            command_id: 'cmd_queue',
          },
        };
        await page.route('**/api/v2/agents/agt_contract/queued-inputs', (route) =>
          route.fulfill({
            json: {
              object: 'cma.agent.queued_inputs',
              agent_id: 'agt_contract',
              next_after: null,
              data: queued
                ? [{ response, command_revision: 7, text: 'Queued request', editable: true }]
                : [],
            },
          }),
        );
        await page.route(
          '**/api/v2/agents/agt_contract/responses/resp_queue/queued-cancellation',
          async (route) => {
            const body = route.request().postDataJSON() as Record<string, unknown>;
            queueWrites.push(body);
            queued = false;
            await route.fulfill({
              json: {
                object: 'cma.response.queued_cancellation',
                cancellation: {
                  ...commandStatus(fixture.started, true),
                  turn_sequence: null,
                  observed_cursor: null,
                  delivery: null,
                },
                response: { ...response, status: 'cancelled' },
                target: { response_id: 'resp_queue', ...body },
              },
            });
          },
        );
        started = performance.now();
        await page.goto('/agents/agt_contract');
        await page.getByText('Recorded request', { exact: true }).waitFor();
        marks.transcript = elapsed();
        await expect.poll(() => fixture.streams.size).toBe(1);
        if (active)
          fixture.append({
            type: 'approval_requested',
            approval_id: 'live',
            action: 'Local approval',
            detail: null,
          });
        const approvalArrival = elapsed();
        const stop = page.getByRole('button', { name: 'Stop response', exact: true });
        const takeover = page.getByRole('button', { name: 'Take control', exact: true });
        const remove = page.getByRole('button', { name: 'Remove queued message 1', exact: true });
        await Promise.all([
          ...(active
            ? [
                expect(stop)
                  .toBeEnabled({ timeout: 70_000 })
                  .then(() => {
                    marks.stop = elapsed();
                  }),
                expect(page.getByRole('button', { name: 'Allow', exact: true }))
                  .toBeEnabled({ timeout: 70_000 })
                  .then(() => {
                    marks.approval = elapsed();
                  }),
              ]
            : []),
          expect(takeover)
            .toBeEnabled({ timeout: 70_000 })
            .then(() => {
              marks.takeover = elapsed();
            }),
          expect(remove)
            .toBeEnabled({ timeout: 70_000 })
            .then(() => {
              marks.queue = elapsed();
            }),
        ]);
        if (!baseline) {
          for (const key of ['stop', 'takeover', 'queue'])
            if (marks[key] !== undefined) expect(marks[key]).toBeLessThan(5000);
          if (active) expect(marks.approval! - approvalArrival).toBeLessThan(1000);
          expect(requests).toHaveLength(1);
          expect(requests[0]!.end).toBeUndefined();
        }
        if (active) {
          await page.getByRole('button', { name: 'Allow', exact: true }).click();
          await expect.poll(() => fixture.writes.length).toBe(1);
          fixture.append({ type: 'approval_resolved', approval_id: 'live', decision: 'allow' });
          await stop.click();
          await expect.poll(() => fixture.writes.length).toBe(2);
          fixture.append({ type: 'turn_cancelled', turn_sequence: 1 });
        }
        await remove.click();
        await expect.poll(() => queueWrites.length).toBe(1);
        await takeover.click();
        await expect(page.getByRole('button', { name: 'Return to Agent' })).toBeEnabled();
        await expect(page.getByText('Loading conversation activity.', { exact: true })).toHaveCount(
          0,
          { timeout: 70_000 },
        );
        marks.historyComplete = elapsed();
        expect(requests).toHaveLength(pages);
        await expect(page.getByRole('button', { name: 'Return to Agent' })).toBeEnabled();
        await expect(remove).toHaveCount(0);
        await expect(stop).toHaveCount(0);
        await expect(page.getByRole('button', { name: 'Allow', exact: true })).toHaveCount(0);
        expect(fixture.writes).toHaveLength(active ? 2 : 0);
        if (active) {
          expect(fixture.writes.map((write) => write.body.command)).toEqual([
            { type: 'resolve_approval', approval_id: 'live', decision: 'allow' },
            { type: 'cancel' },
          ]);
          expect(new Set(fixture.writes.map((write) => write.key)).size).toBe(2);
        }
        expect(fixture.errors).toEqual([]);
        expect(fixture.boundary.errors).toEqual([]);
        expect(fixture.boundary.unexpected).toEqual([]);
      } finally {
        clearInterval(heartbeat);
        await info.attach('control-readiness.json', {
          body: JSON.stringify({
            baseline,
            pages,
            active,
            marks,
            requests,
            queueWrites: queueWrites.length,
            controlWrites: fixture.writes.length,
          }),
          contentType: 'application/json',
        });
        await fixture.close(page);
      }
    });
  }
