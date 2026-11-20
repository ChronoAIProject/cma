import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useQueuedInputs } from './queuedInputsStatus';
import { json } from './controller.fixture';
import {
  deferred,
  reconciliationHarness,
  status,
  unavailable,
} from './queuedReconciliation.fixture';
import { queuePresentation } from '../queuePresentation';
import type { ConversationState } from '../model';

beforeEach(() => {
  sessionStorage.clear();
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});
const advance = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));
const identity = () => false;
const empty: ConversationState = {
  messages: [],
  toolCalls: [],
  reasoning: [],
  approvals: [],
  plans: [],
  recovery: [],
  cursor: 0,
};

it('reconciles nine admitted messages across three discovery pages into six deliveries and three expiries', async () => {
  const h = reconciliationHarness();
  h.setRows([1, 2, 3, 4, 5, 6, 7, 8, 9]);
  const hook = renderHook(() => useQueuedInputs(h.session, true, false, '0', identity));
  await advance();
  expect(h.list.mock.calls.map(([after]) => after)).toEqual([0, 3, 6]);
  expect(hook.result.current.items).toHaveLength(9);
  h.setRows([]);
  h.command.mockImplementation(async (id) => json(status(id, id <= 6 ? 'completed' : 'expired')));
  await advance(4000);
  const view = hook.result.current;
  expect(view.items).toEqual([]);
  expect(view.outcomes.filter((row) => row.kind === 'completed')).toHaveLength(6);
  expect(view.outcomes.filter((row) => row.kind === 'expired')).toHaveLength(3);
  const presented = queuePresentation(
    {
      ...empty,
      messages: [1, 2, 3, 4, 5, 6].map((id) => ({
        id: `event_${id}`,
        commandId: `cmd_${id}`,
        role: 'user',
        content: `Message ${id}`,
        status: 'complete',
      })),
    },
    [],
    view,
  );
  expect(presented.outcomes.map((row) => row.commandId)).toEqual(['cmd_7', 'cmd_8', 'cmd_9']);
  await advance(60000);
  expect(h.command).toHaveBeenCalledTimes(9);
  expect(h.fetch.mock.calls.filter(([, init]) => init?.method === 'POST')).toHaveLength(0);
  hook.unmount();
});

it.each(['queued', 'starting'] as const)(
  'keeps missing %s commands visible through a tail gap and correlates only command IDs',
  async (kind) => {
    const h = reconciliationHarness();
    h.setRows([1]);
    const hook = renderHook(() => useQueuedInputs(h.session, true, false, '0', identity));
    await advance();
    h.setRows([]);
    h.command.mockImplementation(async (id) => json(status(id, kind)));
    await advance(2000);
    const outcome = hook.result.current.outcomes[0]!;
    expect(outcome.kind).toBe(kind === 'queued' ? 'waiting' : 'starting');
    expect(outcome.terminal).toBe(false);
    const other = {
      ...empty,
      messages: [
        {
          id: 'event',
          commandId: 'other',
          role: 'user' as const,
          content: 'Message 1',
          status: 'complete' as const,
        },
      ],
    };
    expect(queuePresentation(other, [], hook.result.current).outcomes).toHaveLength(1);
    expect(
      queuePresentation(
        { ...other, messages: [{ ...other.messages[0]!, commandId: 'cmd_1' }] },
        [],
        hook.result.current,
      ).outcomes,
    ).toEqual([]);
    await advance(10000);
    expect(h.command).toHaveBeenCalledTimes(2);
    hook.unmount();
  },
);

it('retains and fairly drains restored history beyond the batch bound, with at most two command reads in flight', async () => {
  const h = reconciliationHarness();
  h.journal.queuedInputs.remember(Array.from({ length: 25 }, (_, i) => `cmd_${i + 1}`));
  let inFlight = 0;
  let maximum = 0;
  h.command.mockImplementation(async (id) => {
    inFlight++;
    maximum = Math.max(maximum, inFlight);
    await new Promise((resolve) => setTimeout(resolve, 10));
    inFlight--;
    return id <= 8 ? unavailable(503) : json(status(id, 'expired'));
  });
  const hook = renderHook(() => useQueuedInputs(h.session, true, false, '0', identity));
  await advance(100);
  expect(h.command).toHaveBeenCalledTimes(8);
  expect(hook.result.current.managedCommandIds.size).toBe(25);
  await advance(6200);
  expect(new Set(h.command.mock.calls.map(([id]) => id)).size).toBe(25);
  expect(maximum).toBe(2);
  expect(hook.result.current.outcomes).toHaveLength(17);
  expect(hook.result.current.outcomes.every((outcome) => outcome.input === undefined)).toBe(true);
  const terminalReads = h.command.mock.calls.filter(([id]) => id > 8).length;
  await advance(60000);
  expect(h.command.mock.calls.filter(([id]) => id > 8)).toHaveLength(terminalReads);
  expect(hook.result.current.error).toContain('could not be confirmed');
  hook.unmount();
});

it.each([404, 403, 429, 503])(
  'retains known text and unknown status after command HTTP %s, without replay',
  async (code) => {
    const h = reconciliationHarness();
    h.setRows([1]);
    const hook = renderHook(() => useQueuedInputs(h.session, true, false, '0', identity));
    await advance();
    h.setRows([]);
    h.command.mockImplementation(async () => unavailable(code));
    await advance(2000);
    expect(hook.result.current.outcomes[0]).toMatchObject({
      commandId: 'cmd_1',
      input: 'Message 1',
      kind: 'unknown',
      terminal: false,
    });
    await advance(30000);
    expect(h.command.mock.calls.length).toBeGreaterThan(1);
    expect(h.command.mock.calls.length).toBeLessThan(8);
    expect(h.fetch.mock.calls.every(([, init]) => init?.method !== 'POST')).toBe(true);
    hook.unmount();
  },
);

it('does not let discovery failure block known command reconciliation', async () => {
  const h = reconciliationHarness();
  h.journal.queuedInputs.remember(['cmd_1']);
  h.list.mockImplementation(async () => unavailable(404));
  h.command.mockImplementation(async (id) => json(status(id, 'expired')));
  const hook = renderHook(() => useQueuedInputs(h.session, true, false, '0', identity));
  await advance();
  expect(hook.result.current.outcomes[0]?.kind).toBe('expired');
  hook.unmount();
});

it('respects a shared Retry-After floor through refresh and event hints', async () => {
  const h = reconciliationHarness();
  h.journal.queuedInputs.remember(['cmd_1', 'cmd_2', 'cmd_3']);
  h.command.mockImplementation(async () => unavailable(429, '60'));
  const hook = renderHook(
    ({ revision }) => useQueuedInputs(h.session, true, false, revision, identity),
    { initialProps: { revision: '0' } },
  );
  await advance();
  const count = h.command.mock.calls.length;
  expect(count).toBeLessThanOrEqual(2);
  for (let i = 0; i < 20; i++) {
    hook.rerender({ revision: String(i + 1) });
    act(() => hook.result.current.refresh());
    await advance(2000);
  }
  expect(h.command).toHaveBeenCalledTimes(count);
  await advance(20000);
  expect(h.command.mock.calls.length).toBeGreaterThan(count);
  hook.unmount();
});

it.each(['agent', 'response', 'command', 'null-response'])(
  'rejects mismatched %s evidence for an absent command',
  async (mismatch) => {
    const h = reconciliationHarness();
    const accepted = h.journal.retain('Original');
    h.journal.accepted(accepted.key, { commandId: 'cmd_1', responseId: 'resp_1' });
    const value = status(1, 'expired');
    h.command.mockImplementation(async () =>
      json({
        ...value,
        receipt: {
          ...value.receipt,
          ...(mismatch === 'agent'
            ? { agent_id: 'another-agent' }
            : mismatch === 'response'
              ? { response_id: 'another-response' }
              : mismatch === 'command'
                ? { command_id: 'cmd_2' }
                : { response_id: null }),
        },
      }),
    );
    const hook = renderHook(() => useQueuedInputs(h.session, true, false, '0', identity));
    await advance();
    expect(hook.result.current.outcomes[0]).toMatchObject({ kind: 'unknown', terminal: false });
    hook.unmount();
  },
);

it('discards a late status after the session is replaced or access is disabled', async () => {
  const h = reconciliationHarness();
  h.journal.queuedInputs.remember(['cmd_1']);
  const pending = deferred<Response>();
  h.command.mockReturnValue(pending.promise);
  const hook = renderHook(
    ({ enabled, session }) => useQueuedInputs(session, enabled, false, '0', identity),
    { initialProps: { enabled: true, session: h.session } },
  );
  await advance();
  hook.rerender({ enabled: false, session: h.session });
  await act(async () => pending.resolve(json(status(1, 'expired'))));
  expect(hook.result.current.outcomes).toEqual([]);
  sessionStorage.clear();
  const next = reconciliationHarness();
  hook.rerender({ enabled: true, session: next.session });
  await advance();
  expect(hook.result.current.outcomes).toEqual([]);
  expect(next.command).not.toHaveBeenCalled();
  hook.unmount();
});

it('requires authorization after reading a restored command before publishing its outcome', async () => {
  const h = reconciliationHarness();
  h.journal.queuedInputs.remember(['cmd_1']);
  h.command.mockImplementation(async (id) => {
    h.currentUser.mockResolvedValue('bob');
    return json(status(id, 'expired'));
  });
  const changed = vi.fn(() => true);
  const hook = renderHook(() => useQueuedInputs(h.session, true, false, '0', changed));
  await advance();
  expect(hook.result.current.outcomes).toEqual([]);
  expect(changed).toHaveBeenCalledOnce();
  await advance(60000);
  expect(h.command).toHaveBeenCalledOnce();
  hook.unmount();
});
