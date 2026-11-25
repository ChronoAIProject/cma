import { beforeEach, expect, it, vi } from 'vitest';
import fixture from '../../test/externalApprovalProjection.fixture.json';
import { binding, cursor, incarnation, item, page } from './controller.fixture';
import { setup } from './load.test.fixture';
import { loadV2Agent } from './load';

beforeEach(() => sessionStorage.clear());
const rows = [
  item(1, { type: 'turn_started', turn_sequence: 1 }, binding),
  item(2, { type: 'message', role: 'user', content: 'Synthetic original question' }),
  item(3, { ...fixture.live_requested.data.kind, turn_sequence: 1 }),
];
const response = (value: unknown, headers = fixture.response_headers) =>
  Response.json(value, { headers });

it('hydrates a negotiated approval across fixed pages with its original turn binding', async () => {
  const h = await setup();
  h.snapshot
    .mockResolvedValueOnce(response(page(rows.slice(0, 2), 3, 2)))
    .mockResolvedValueOnce(response(page(rows.slice(2), 3)));
  const result = await h.start();
  expect(result.error).toBeUndefined();
  expect(result.value?.conversation.approvals).toMatchObject([{ source: 'external', runId: '1' }]);
  expect(result.value?.conversation.responseBinding?.commandId).toBe('cmd_1');
  expect(h.post).not.toHaveBeenCalled();
});

it('refuses a changed page policy and invalidates any earlier live action authority', async () => {
  const h = await setup();
  h.snapshot.mockResolvedValueOnce(response(page(rows.slice(0, 2), 3, 2))).mockResolvedValueOnce(
    response(page(rows.slice(2), 3), {
      ...fixture.response_headers,
      'CMA-External-Approval-Action': 'none',
    }),
  );
  const result = await h.start();
  expect(result.error).toEqual(
    expect.objectContaining({ message: expect.stringContaining('Approval access changed') }),
  );
  expect(h.v2.approvalProjectionAvailable).toBe(false);
  expect(h.post).not.toHaveBeenCalled();
});

it('compares the independently negotiated head policy against the completed snapshot', async () => {
  const h = await setup();
  h.snapshot.mockResolvedValue(response(page(rows, 3)));
  h.head.mockResolvedValue(
    response(
      {
        object: 'cma.agent.event_head',
        schema_version: 1,
        agent_id: 'agt_1',
        incarnation,
        through_cursor: cursor(3),
        active_turn: rows[0],
      },
      { ...fixture.response_headers, 'CMA-External-Approval-Action': 'none' },
    ),
  );
  await expect(
    loadV2Agent(h.client, 'agt_1', h.abort.signal, h.v2, false, undefined, undefined, vi.fn()),
  ).rejects.toThrow('Approval access changed');
  expect(h.v2.approvalProjectionAvailable).toBe(false);
  expect(h.post).not.toHaveBeenCalled();
});
