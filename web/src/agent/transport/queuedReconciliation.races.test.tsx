import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useQueuedInputs } from './queuedInputsStatus';
import {
  deferred,
  entry,
  reconciliationHarness,
  status,
  unavailable,
} from './queuedReconciliation.fixture';
import { json, accepted } from './controller.fixture';
import { queuedReceipt } from './queued.fixture';
import { commandStatus } from './controls.fixture';
import type { ConversationMessage } from '../model';

beforeEach(() => {
  sessionStorage.clear();
  vi.useFakeTimers();
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});
const advance = (ms = 0) => act(() => vi.advanceTimersByTimeAsync(ms));
const identity = () => false;
const page = () =>
  json({
    object: 'cma.agent.queued_inputs',
    agent_id: 'agt_1',
    data: [entry(1)],
    next_after: null,
  });
function mutationHarness() {
  const h = reconciliationHarness();
  h.setRows([1]);
  let lost = false;
  const original = h.fetch.getMockImplementation()!;
  h.fetch.mockImplementation(async (url, init) => {
    if (String(url).endsWith('/queued-cancellation')) {
      const body = JSON.parse(String(init?.body));
      h.setRows([]);
      if (lost) {
        lost = false;
        throw new TypeError('Lost reply');
      }
      return json(queuedReceipt('resp_1', body));
    }
    if (String(url).endsWith('/queued-input')) {
      const body = JSON.parse(String(init?.body));
      return json({
        object: 'cma.response.queued_input',
        edit: {
          ...commandStatus(null),
          status: 'completed',
          turn_sequence: null,
          observed_cursor: null,
          delivery: null,
        },
        response: status(1),
        target: {
          response_id: 'resp_1',
          source_command_id: 'cmd_1',
          expected_command_revision: body.expected_command_revision,
        },
        command_revision: body.expected_command_revision + 1,
        text: body.text,
      });
    }
    return original(url, init);
  });
  return {
    ...h,
    loseReply: () => {
      lost = true;
    },
  };
}
it.each(['discovery', 'command'])(
  'cannot restore a cancelled message from late %s evidence',
  async (source) => {
    const h = mutationHarness();
    const hook = renderHook(() => useQueuedInputs(h.session, true, false, '0', identity));
    await advance();
    const row = hook.result.current.items[0]!;
    const late = deferred<Response>();
    if (source === 'discovery') h.list.mockReturnValueOnce(late.promise);
    else {
      h.setRows([]);
      h.command.mockReturnValueOnce(late.promise);
    }
    await advance(2000);
    await act(() => hook.result.current.remove(row));
    expect(hook.result.current.outcomes[0]?.kind).toBe('cancelled');
    await act(async () => late.resolve(source === 'discovery' ? page() : json(status(1))));
    await advance(100);
    expect(hook.result.current.items).toEqual([]);
    expect(hook.result.current.outcomes[0]?.kind).toBe('cancelled');
    h.setRows([1]);
    act(() => hook.result.current.refresh());
    await advance(100);
    expect(hook.result.current.items).toEqual([]);
    expect(hook.result.current.outcomes[0]?.kind).toBe('cancelled');
    hook.unmount();
  },
);
it('preserves a confirmed edit revision through both late and stale discovery', async () => {
  const h = mutationHarness();
  const hook = renderHook(() => useQueuedInputs(h.session, true, false, '0', identity));
  await advance();
  const late = deferred<Response>();
  h.list.mockReturnValueOnce(late.promise);
  await advance(2000);
  await act(() => hook.result.current.edit(hook.result.current.items[0]!, 'Revised instruction'));
  await act(async () => late.resolve(page()));
  await advance(2100);
  expect(hook.result.current.items[0]).toMatchObject({ revision: 8, input: 'Revised instruction' });
  hook.unmount();
});
it('reuses the original cancellation key and body after an inconclusive read and reload', async () => {
  const h = mutationHarness();
  h.loseReply();
  h.command.mockImplementation(async () => json(status(1, 'starting')));
  const first = renderHook(() => useQueuedInputs(h.session, true, false, '0', identity));
  await advance();
  await act(async () => {
    await expect(first.result.current.remove(first.result.current.items[0]!)).rejects.toThrow(
      'Lost reply',
    );
  });
  const intent = h.journal.queuedInputs.read()[0]!;
  first.unmount();
  const second = renderHook(() => useQueuedInputs(h.session, true, false, '0', identity));
  await advance();
  const queue = second.result.current;
  expect(queue.items[0]?.needsRecovery).toBe(true);
  await act(() => second.result.current.recover(queue.items[0]!));
  expect(h.journal.queuedInputs.read()).toEqual([]);
  const writes = h.fetch.mock.calls.filter(([, init]) => init?.method === 'POST');
  expect(writes).toHaveLength(2);
  expect(writes[1]?.[1]?.body).toBe(writes[0]?.[1]?.body);
  expect(new Headers(writes[1]?.[1]?.headers).get('idempotency-key')).toBe(intent.key);
  expect(second.result.current.items).toEqual([]);
  expect(second.result.current.outcomes[0]?.kind).toBe('cancelled');
  second.unmount();
});

function removal(id: number, key = `key_${id}`) {
  return {
    key,
    responseId: `resp_${id}`,
    commandId: `cmd_${id}`,
    revision: 7,
    original: 'Same message',
    action: 'remove' as const,
  };
}

it.each(['cancelled', 'completed', 'expired'] as const)(
  'clears uncertain cancellation after a verified %s read on reload',
  async (outcome) => {
    const h = mutationHarness();
    h.loseReply();
    const first = renderHook(() => useQueuedInputs(h.session, true, false, '0', identity));
    await advance();
    await act(async () => {
      await expect(first.result.current.remove(first.result.current.items[0]!)).rejects.toThrow(
        'Lost reply',
      );
    });
    expect(h.journal.queuedInputs.read()).toHaveLength(1);
    first.unmount();
    h.command.mockImplementation(async () => json(status(1, outcome)));
    const second = renderHook(() => useQueuedInputs(h.session, true, false, '0', identity));
    await advance();
    expect(h.journal.queuedInputs.read()).toEqual([]);
    expect(second.result.current.items).toEqual([]);
    expect(second.result.current.outcomes[0]).toMatchObject({ kind: outcome, terminal: true });
    expect(h.fetch.mock.calls.filter(([, init]) => init?.method === 'POST')).toHaveLength(1);
    second.unmount();
  },
);

it('observes and resolves an older legacy cancellation even when another input is current', async () => {
  const h = reconciliationHarness();
  h.journal.queuedCancellation.retain({
    key: 'legacy_key',
    responseId: 'resp_1',
    commandId: null,
    body: { source_command_id: 'cmd_1', expected_command_revision: 7 },
  });
  h.setRows([2]);
  h.command.mockImplementation(async (id) => json(status(id, 'cancelled')));
  const hook = renderHook(() => useQueuedInputs(h.session, true, false, '0', identity));
  await advance();
  expect(h.journal.queuedCancellation.read()).toBeNull();
  expect(hook.result.current.items.map((item) => item.commandId)).toEqual(['cmd_2']);
  expect(hook.result.current.outcomes[0]).toMatchObject({ commandId: 'cmd_1', kind: 'cancelled' });
  expect(h.fetch.mock.calls.some(([, init]) => init?.method === 'POST')).toBe(false);
  hook.unmount();
});

it('reads a pending cancellation through discovery failure and keeps it until the exact source confirms', async () => {
  const h = mutationHarness();
  h.loseReply();
  const hook = renderHook(() => useQueuedInputs(h.session, true, true, '0', identity));
  await advance();
  h.list.mockImplementation(async () => unavailable(503));
  h.command.mockImplementation(async () => unavailable(503));
  await act(async () => {
    await expect(hook.result.current.remove(hook.result.current.items[0]!)).rejects.toThrow(
      'Lost reply',
    );
  });
  const original = h.journal.queuedInputs.read()[0]!;
  await advance(2000);
  expect(h.command).toHaveBeenCalledWith(1);
  expect(h.journal.queuedInputs.read()).toEqual([original]);
  expect(hook.result.current.items[0]?.needsRecovery).toBe(true);

  h.command.mockImplementation(async () => json(status(1, 'cancelled')));
  await advance(10000);
  expect(h.journal.queuedInputs.read()).toEqual([]);
  expect(hook.result.current.items).toEqual([]);
  expect(hook.result.current.outcomes[0]).toMatchObject({ kind: 'cancelled', terminal: true });
  expect(h.fetch.mock.calls.filter(([, init]) => init?.method === 'POST')).toHaveLength(1);
  hook.unmount();
});

it('uses exact durable user-message identity and keeps a second cancellation with identical text', async () => {
  const h = reconciliationHarness();
  h.journal.queuedInputs.retain(removal(1));
  h.journal.queuedInputs.retain(removal(2));
  h.setRows([1, 2]);
  const message: ConversationMessage = {
    id: 'event:3',
    role: 'user',
    content: 'Same message',
    status: 'complete',
    commandId: 'cmd_1',
    responseId: 'resp_2',
  };
  const hook = renderHook(
    ({ item }: { item: ConversationMessage }) =>
      useQueuedInputs(h.session, true, false, item.responseId!, identity, 0, [item]),
    { initialProps: { item: message } },
  );
  await advance();
  expect(h.journal.queuedInputs.read()).toEqual([removal(1), removal(2)]);
  hook.rerender({ item: { ...message, responseId: 'resp_1' } });
  await advance(2000);
  expect(h.journal.queuedInputs.read()).toEqual([removal(2)]);
  expect(hook.result.current.items.map((item) => item.commandId)).toEqual(['cmd_2']);
  expect(h.fetch.mock.calls.some(([, init]) => init?.method === 'POST')).toBe(false);
  hook.unmount();
});

it('a late terminal read does not clear a replacement intent for the same source', async () => {
  const h = reconciliationHarness();
  const original = h.journal.queuedInputs.retain(removal(1));
  const late = deferred<Response>();
  h.command.mockReturnValueOnce(late.promise);
  const hook = renderHook(() => useQueuedInputs(h.session, true, false, '0', identity));
  await advance();
  expect(h.command).toHaveBeenCalledTimes(1);
  h.journal.queuedInputs.settle(original);
  const replacement = h.journal.queuedInputs.retain(removal(1, 'replacement'));
  await act(async () => late.resolve(json(status(1, 'completed'))));
  expect(h.journal.queuedInputs.read()).toEqual([replacement]);
  expect(hook.result.current.items[0]?.needsRecovery).toBe(true);
  hook.unmount();
});
it('retains discovered commands in memory when status persistence is refused', async () => {
  const h = reconciliationHarness();
  h.setRows([1]);
  vi.spyOn(h.journal.queuedInputs, 'remember').mockImplementation(() => {
    throw new Error('storage unavailable');
  });
  const hook = renderHook(() => useQueuedInputs(h.session, true, false, '0', identity));
  await advance();
  expect(hook.result.current.items).toHaveLength(1);
  expect(hook.result.current.error).toContain('could not be saved');
  h.setRows([]);
  h.command.mockImplementation(async () => json(status(1, 'expired')));
  await advance(2000);
  expect(hook.result.current.outcomes[0]).toMatchObject({ kind: 'expired', input: 'Message 1' });
  hook.unmount();
});
it('retains every accepted receipt when multiple Sends finish before the next observation', async () => {
  const h = reconciliationHarness();
  const original = h.fetch.getMockImplementation()!;
  let sequence = 0;
  h.fetch.mockImplementation((url, init) => {
    if (String(url).endsWith('/responses')) {
      const id = ++sequence;
      const response = accepted();
      response.headers.set('x-cma-command-id', `cmd_${id}`);
      response.headers.set('x-cma-response-id', `resp_${id}`);
      return Promise.resolve(response);
    }
    return original(url, init);
  });
  for (let id = 1; id <= 9; id++) await h.session.send(`Message ${id}`, () => {});
  expect(h.journal.queuedInputs.managedCommands().size).toBe(9);
  h.command.mockImplementation(async (id) => json(status(id, 'expired')));
  const hook = renderHook(() => useQueuedInputs(h.session, true, false, '0', identity));
  await advance(2000);
  expect(hook.result.current.outcomes).toHaveLength(9);
  expect(hook.result.current.outcomes.map((row) => row.input)).toEqual(
    Array.from({ length: 9 }, (_, i) => `Message ${i + 1}`),
  );
  expect(h.fetch.mock.calls.filter(([, init]) => init?.method === 'POST')).toHaveLength(9);
  hook.unmount();
});
