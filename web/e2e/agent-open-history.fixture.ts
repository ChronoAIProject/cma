import { expect, type Page } from '@playwright/test';
import { cursor, LocalAgentV2 } from './agent-v2.fixture';

const incarnation = 'a'.repeat(64);
const maximumBytes = 4 * 1024 * 1024;

/** Fixed-prefix pagination with the public server's row and byte bounds. */
export async function openingHistory(page: Page, baseURL: string, byteBound: boolean) {
  const fixture = new LocalAgentV2();
  fixture.append({ type: 'turn_started', turn_sequence: 1 });
  fixture.append({ type: 'message', role: 'user', content: 'Recorded history request' });
  fixture.append({
    type: 'tool_started',
    call_id: 'history_tool',
    title: 'Recorded history tool',
    kind: 'execute',
    status: 'completed',
  });
  for (let index = 3; index < 203; index++)
    fixture.append(
      byteBound && index < 9
        ? {
            type: 'tool_output_reported',
            call_id: 'history_tool',
            output: { text: 'x'.repeat(900 * 1024), truncated: false },
          }
        : { type: 'conversation_available' },
    );
  fixture.append({ type: 'message', role: 'assistant', content: 'Previous recorded answer' });
  fixture.append({ type: 'turn_completed', turn_sequence: 1 });
  const items = [...fixture.items];
  const through = items.length;
  const pages: { after: number; count: number; bytes: number; limit: number }[] = [];
  let release!: () => void;
  const continuation = new Promise<void>((resolve) => {
    release = resolve;
  });
  let continuationRequested!: () => void;
  const waiting = new Promise<void>((resolve) => {
    continuationRequested = resolve;
  });
  await fixture.route(page, baseURL);
  await page.route('**/api/v2/agents/agt_contract/events/head', (route) =>
    route.fulfill({
      json: {
        object: 'cma.agent.event_head',
        schema_version: 1,
        agent_id: 'agt_contract',
        incarnation,
        through_cursor: cursor(through),
        active_turn: null,
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
            owner_message: 'Recorded history request',
            agent_messages: ['Previous recorded answer'],
            messages: [items[1]!, items[203]!].map((item) => ({
              sequence: item.sequence,
              role: item.kind.role,
              content: item.kind.content,
              appended_at: fixture.started,
            })),
            outcome: { type: 'completed', at: fixture.started },
          },
        ],
      },
    }),
  );
  await page.route('**/api/v2/agents/agt_contract/events/snapshot**', async (route) => {
    const query = new URL(route.request().url()).searchParams;
    const limit = Number(query.get('limit'));
    expect(limit).toBe(200);
    const after = pages.reduce((count, entry) => count + entry.count, 0);
    expect(query.get('page_cursor')).toBe(
      after === 0 ? null : `ags1.${incarnation}.${through}.${after}`,
    );
    if (after > 0) {
      continuationRequested();
      await continuation;
    }
    let used = 2048;
    const selected = [];
    for (const item of items.slice(after, after + limit)) {
      const size = Buffer.byteLength(JSON.stringify(item)) + 1;
      if (used + size > maximumBytes) break;
      used += size;
      selected.push(item);
    }
    expect(selected.length).toBeGreaterThan(0);
    const next = after + selected.length;
    const body = JSON.stringify({
      object: 'cma.agent.event_page',
      schema_version: 1,
      agent_id: 'agt_contract',
      incarnation,
      through_cursor: cursor(through),
      items: selected,
      next_page_cursor: next < through ? `ags1.${incarnation}.${through}.${next}` : null,
    });
    expect(Buffer.byteLength(body)).toBeLessThanOrEqual(maximumBytes);
    pages.push({ after, count: selected.length, bytes: Buffer.byteLength(body), limit });
    await route.fulfill({ contentType: 'application/json', body });
  });
  return { fixture, through, pages, release, waiting };
}
