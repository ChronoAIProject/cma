import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, AGENT_USAGE } from '../../test/agent';
import { workspaceFixture } from '../../test/hierarchy';
import { cursor, harness, incarnation, json, page } from './controller.fixture';
import { loadV2Agent } from './load';

beforeEach(() => sessionStorage.clear());
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('authorizes independent completed projections concurrently, then checks again after their callbacks', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  vi.useFakeTimers();
  const delay = <Value>(ms: number, value: Value) =>
    new Promise<Value>((resolve) => setTimeout(() => resolve(value), ms));
  h.currentUser.mockClear().mockImplementation(() => delay(250, 'alice'));
  vi.spyOn(h.client, 'readAgent').mockImplementation(() => delay(300, AGENT_DETAIL));
  vi.spyOn(h.client, 'readWorkspace').mockImplementation(() => delay(300, workspaceFixture()));
  vi.spyOn(h.client, 'readAgentUsage').mockImplementation(() => delay(300, AGENT_USAGE));
  vi.spyOn(h.client, 'readAgentTranscript').mockImplementation(() =>
    delay(300, { ...AGENT_TRANSCRIPT, turns: [], throughSeq: 0 }),
  );
  h.snapshot.mockImplementation(() => delay(300, json(page([], 0))));
  h.head.mockImplementation(() =>
    delay(
      300,
      json({
        object: 'cma.agent.event_head',
        schema_version: 1,
        agent_id: 'agt_1',
        incarnation,
        through_cursor: cursor(0),
        active_turn: null,
      }),
    ),
  );
  const input = vi.fn();
  const settled = vi.fn();
  const result = loadV2Agent(
    h.client,
    'agt_1',
    new AbortController().signal,
    session,
    false,
    undefined,
    {
      agentId: AGENT_DETAIL.id,
      workspaceId: AGENT_DETAIL.workspace.id,
      sandboxId: AGENT_DETAIL.sandbox.id,
    },
    input,
  ).then((value) => {
    settled();
    return value;
  });
  await vi.advanceTimersByTimeAsync(250);
  expect(h.currentUser).toHaveBeenCalledOnce();
  expect(input).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(300);
  // Each check begins after its own data is complete. None borrows an earlier
  // in-flight identity request or waits for an unrelated publication check.
  expect(h.currentUser).toHaveBeenCalledTimes(4);
  expect(input).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(250);
  expect(input).toHaveBeenCalledTimes(3);
  expect(input.mock.calls.some(([ready]) => ready.conversation.agentCursor === cursor(0))).toBe(
    true,
  );
  expect(h.currentUser).toHaveBeenCalledTimes(5);
  expect(settled).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(249);
  expect(settled).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect((await result).conversation.agentCursor).toBe(cursor(0));
  expect(settled).toHaveBeenCalledOnce();
});

it('rejects an account switch made by an early publication before releasing the full snapshot', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  h.head.mockResolvedValue(
    json({
      object: 'cma.agent.event_head',
      schema_version: 1,
      agent_id: 'agt_1',
      incarnation,
      through_cursor: cursor(0),
      active_turn: null,
    }),
  );
  const input = vi.fn(() => h.currentUser.mockResolvedValue('bob'));
  await expect(
    loadV2Agent(
      h.client,
      'agt_1',
      new AbortController().signal,
      session,
      false,
      undefined,
      undefined,
      input,
    ),
  ).rejects.toMatchObject({ name: 'IdentityChangedError' });
  expect(input).toHaveBeenCalled();
  expect(sessionStorage.length).toBe(0);
  expect(h.post).not.toHaveBeenCalled();
});
