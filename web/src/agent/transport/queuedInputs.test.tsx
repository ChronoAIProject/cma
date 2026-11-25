import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { createClient } from '@cma/client';
import { AgentV2Session } from './client';
import { agentTransportJournal, clearAgentTransport } from './journal';
import { useQueuedInputs } from './queuedInputsStatus';
import { queuedObservation, queuedReceipt } from './queued.fixture';
import { commandStatus } from './controls.fixture';
import { json } from './controller.fixture';

beforeEach(() => sessionStorage.clear());
const intent = (id: string) => ({
  key: `key_${id}`,
  responseId: `resp_${id}`,
  commandId: `cmd_${id}`,
  revision: 7,
  original: `original ${id}`,
  action: 'edit' as const,
  text: `changed ${id}`,
});
function journal() {
  const value = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
  value.select(true);
  return value;
}
it('retains independent uncertain edits across reload and settles only the exact target', () => {
  const first = journal();
  first.queuedInputs.retain(intent('one'));
  first.queuedInputs.retain(intent('two'));
  const reload = journal();
  expect(reload.queuedInputs.read()).toEqual([intent('one'), intent('two')]);
  expect(() => reload.queuedInputs.retain({ ...intent('one'), text: 'different' })).toThrow(
    'Confirm',
  );
  expect(() => reload.queuedInputs.settle({ ...intent('one'), key: 'wrong' })).toThrow('identity');
  reload.queuedInputs.settle(intent('one'));
  expect(first.queuedInputs.read()).toEqual([intent('two')]);
});
it('invalidates queued drafts on account change and refuses late receipt writes', () => {
  const first = journal();
  first.queuedInputs.retain(intent('one'));
  first.queuedInputs.retain(intent('two'));
  first.queuedInputs.settle(intent('two'), true);
  clearAgentTransport(sessionStorage);
  const second = agentTransportJournal(sessionStorage, 'bob', 'agt_1');
  second.select(true);
  expect(() => first.queuedInputs.settle(intent('one'))).toThrow('sign-in changed');
  expect(second.queuedInputs.read()).toEqual([]);
  expect(second.queuedInputs.unappliedEdits()).toEqual([]);
});

it('keeps the original recovery request if the unapplied draft cannot be saved', () => {
  const first = journal();
  first.queuedInputs.retain(intent('one'));
  const setItem = vi.spyOn(Storage.prototype, 'setItem').mockImplementationOnce(() => {
    throw new Error('Out of space');
  });
  try {
    expect(() => first.queuedInputs.settle(intent('one'), true)).toThrow('Out of space');
    expect(first.queuedInputs.read()).toEqual([intent('one')]);
    expect(first.queuedInputs.unappliedEdits()).toEqual([]);
  } finally {
    setItem.mockRestore();
  }
});

function queueHarness(observedCommandStatus: 'queued' | 'cancelled' = 'cancelled') {
  let rows = ['one', 'two', 'three'].map((id, index) => {
    const response = queuedObservation(`resp_${id}`).response;
    return {
      response: {
        ...response,
        receipt: { ...response.receipt, command_id: `cmd_${id}`, agent_order: index + 1 },
      },
      command_revision: 7,
      text: `original ${id}`,
      editable: true,
    };
  });
  let loseEdit = false;
  let conflictEdit = false;
  const completedEdits = new Map<string, unknown>();
  const fetch = vi.fn<typeof globalThis.fetch>(async (url, init) => {
    if (String(url).endsWith('/queued-inputs'))
      return json({
        object: 'cma.agent.queued_inputs',
        agent_id: 'agt_1',
        data: rows,
        next_after: null,
      });
    if (String(url).includes('/commands/')) {
      const commandId = String(url).split('/').at(-1)!;
      const source = queuedObservation(commandId.replace('cmd_', 'resp_')).response;
      return json({
        ...source,
        receipt: { ...source.receipt, command_id: commandId },
        status: observedCommandStatus,
      });
    }
    const id = String(url).split('/').at(-2)!;
    const body = JSON.parse(String(init?.body));
    const key = new Headers(init?.headers).get('idempotency-key')!;
    const original = rows.find((entry) => entry.response.receipt.response_id === id);
    if (String(url).endsWith('/queued-cancellation')) {
      rows = rows.filter((entry) => entry !== original);
      const receipt = queuedReceipt(id, body);
      receipt.response.receipt.command_id = body.source_command_id;
      return json(receipt);
    }
    if (String(url).endsWith('/queued-input')) {
      if (conflictEdit) {
        conflictEdit = false;
        return json(
          {
            error: {
              type: 'invalid_request_error',
              code: 'queued_input_conflict',
              message: 'Conflict.',
              param: null,
            },
          },
          409,
        );
      }
      if (completedEdits.has(key)) return json(completedEdits.get(key));
      if (!original || original.command_revision !== body.expected_command_revision)
        return json(
          {
            error: {
              type: 'invalid_request_error',
              code: 'queued_input_not_admitted',
              message: 'Message already changed.',
              param: null,
            },
          },
          409,
        );
      const response = {
        object: 'cma.response.queued_input',
        edit: {
          ...commandStatus(null),
          status: 'completed',
          turn_sequence: null,
          observed_cursor: null,
          delivery: null,
        },
        response: original.response,
        target: {
          response_id: id,
          source_command_id: body.source_command_id,
          expected_command_revision: body.expected_command_revision,
        },
        command_revision: body.expected_command_revision + 1,
        text: body.text,
      };
      rows = rows.map((entry) =>
        entry === original
          ? { ...entry, text: body.text, command_revision: entry.command_revision + 1 }
          : entry,
      );
      completedEdits.set(key, response);
      if (loseEdit) {
        loseEdit = false;
        throw new TypeError('lost reply');
      }
      return json(response);
    }
    throw new Error('Unexpected request');
  });
  const session = new AgentV2Session(createClient({ fetch }), journal(), 'alice', {
    storage: sessionStorage,
    currentUser: async () => 'alice',
  });
  return {
    session,
    fetch,
    loseEdit: () => {
      loseEdit = true;
    },
    conflictEdit: () => {
      conflictEdit = true;
    },
    changeSource: () => {
      rows = rows.map((row) =>
        row.response.receipt.command_id === 'cmd_one'
          ? { ...row, command_revision: 8, text: 'other edit' }
          : row,
      );
    },
    rows: () => rows,
  };
}
const identity = () => false;
it('edits an entry in place including attachment paths, then removes another without changing neighbors', async () => {
  const h = queueHarness();
  const hook = renderHook(() => useQueuedInputs(h.session, true, true, '0', identity));
  await waitFor(() => expect(hook.result.current.items).toHaveLength(3));
  const second = hook.result.current.items[1]!;
  const text = 'Use /home/attachments/new-image.png and keep this revised instruction.';
  await act(() => hook.result.current.edit(second, text));
  await waitFor(() => expect(hook.result.current.items[1]?.revision).toBe(8));
  await act(() => hook.result.current.remove(hook.result.current.items[0]!));
  expect(hook.result.current.items.map((x) => x.input)).toEqual([text, 'original three']);
  expect(h.rows().map((x) => x.response.receipt.agent_order)).toEqual([2, 3]);
  hook.unmount();
});
it('recovers a lost edit reply with the same key/body after reload while another entry remains removable', async () => {
  const h = queueHarness();
  h.loseEdit();
  const first = renderHook(() => useQueuedInputs(h.session, true, true, '0', identity));
  await waitFor(() => expect(first.result.current.items).toHaveLength(3));
  const original = first.result.current.items[0]!;
  await act(async () => {
    await expect(first.result.current.edit(original, 'revised one')).rejects.toThrow('lost reply');
  });
  expect(first.result.current.items[0]?.needsRecovery).toBe(true);
  expect(first.result.current.items[0]?.pendingEdit).toBe('revised one');
  await act(() => first.result.current.remove(first.result.current.items[1]!));
  expect(first.result.current.items.map((x) => x.commandId)).toEqual(['cmd_one', 'cmd_three']);
  first.unmount();
  const second = renderHook(() => useQueuedInputs(h.session, true, true, '0', identity));
  await waitFor(() => expect(second.result.current.items).toHaveLength(2));
  expect(second.result.current.items[0]?.pendingEdit).toBe('revised one');
  expect(second.result.current.items[1]?.pendingEdit).toBeUndefined();
  await act(() => second.result.current.recover(second.result.current.items[0]!));
  const edits = h.fetch.mock.calls.filter(([url]) => String(url).endsWith('/queued-input'));
  expect(edits).toHaveLength(2);
  expect(edits[1]?.[1]?.body).toBe(edits[0]?.[1]?.body);
  expect(new Headers(edits[1]?.[1]?.headers).get('idempotency-key')).toBe(
    new Headers(edits[0]?.[1]?.headers).get('idempotency-key'),
  );
  expect(second.result.current.items[0]?.needsRecovery).toBe(false);
  expect(second.result.current.items[0]?.pendingEdit).toBeUndefined();
  expect(h.rows().map((x) => x.text)).toEqual(['revised one', 'original three']);
  second.unmount();
});

it('retains the draft when same-key recovery confirms a stale edit and allows another edit', async () => {
  const h = queueHarness('queued');
  h.conflictEdit();
  const fetchEdits = () =>
    h.fetch.mock.calls.filter(([url]) => String(url).endsWith('/queued-input'));
  const first = renderHook(() => useQueuedInputs(h.session, true, true, '0', identity));
  await waitFor(() => expect(first.result.current.items).toHaveLength(3));
  await act(async () => {
    await expect(
      first.result.current.edit(first.result.current.items[0]!, 'my draft'),
    ).rejects.toThrow('Conflict');
  });
  expect(first.result.current.items[0]?.pendingEdit).toBe('my draft');
  expect(first.result.current.unappliedEdits).toEqual([]);
  first.unmount();

  h.changeSource();
  const second = renderHook(() => useQueuedInputs(h.session, true, true, '0', identity));
  await waitFor(() => expect(second.result.current.items[0]?.revision).toBe(8));
  expect(fetchEdits()).toHaveLength(1);
  await act(() => second.result.current.recover(second.result.current.items[0]!));
  const [save, recover] = fetchEdits();
  expect(fetchEdits()).toHaveLength(2);
  expect(recover?.[1]?.body).toBe(save?.[1]?.body);
  const key = new Headers(save?.[1]?.headers).get('idempotency-key');
  expect(new Headers(recover?.[1]?.headers).get('idempotency-key')).toBe(key);
  expect(second.result.current.items[0]).toMatchObject({
    needsRecovery: false,
    input: 'other edit',
  });
  expect(second.result.current.unappliedEdits).toEqual([{ key, text: 'my draft' }]);
  expect(h.rows()[0]).toMatchObject({ command_revision: 8, text: 'other edit' });
  second.unmount();

  const third = renderHook(
    ({ enabled }) => useQueuedInputs(h.session, enabled, true, '0', identity),
    {
      initialProps: { enabled: true },
    },
  );
  expect(third.result.current.unappliedEdits).toEqual([{ key, text: 'my draft' }]);
  third.rerender({ enabled: false });
  expect(third.result.current.unappliedEdits).toEqual([]);
  third.rerender({ enabled: true });
  await waitFor(() => expect(third.result.current.items[0]?.revision).toBe(8));
  expect(fetchEdits()).toHaveLength(2);
  await act(() => third.result.current.edit(third.result.current.items[0]!, 'my draft'));
  const fresh = fetchEdits()[2];
  expect(JSON.parse(String(fresh?.[1]?.body))).toMatchObject({
    expected_command_revision: 8,
    text: 'my draft',
  });
  expect(new Headers(fresh?.[1]?.headers).get('idempotency-key')).not.toBe(key);
  expect(h.rows()[0]).toMatchObject({ command_revision: 9, text: 'my draft' });
  act(() => third.result.current.dismissUnappliedEdit!(key!));
  expect(third.result.current.unappliedEdits).toEqual([]);
  expect(journal().queuedInputs.unappliedEdits()).toEqual([]);
  third.unmount();
});

it('projects only matching saved edits and hides them while access is suspended', async () => {
  const h = queueHarness('queued');
  h.session.journal.queuedInputs.retain({ ...intent('one'), commandId: 'foreign_command' });
  h.session.journal.queuedInputs.retain(intent('two'));
  h.session.journal.queuedInputs.retain({ ...intent('three'), action: 'remove', text: undefined });
  const hook = renderHook(
    ({ enabled }) => useQueuedInputs(h.session, enabled, true, '0', identity),
    {
      initialProps: { enabled: true },
    },
  );
  await waitFor(() =>
    expect(hook.result.current.items.some((row) => row.commandId === 'cmd_one')).toBe(true),
  );
  for (const id of ['one', 'two', 'three'])
    expect(
      hook.result.current.items.find((row) => row.commandId === `cmd_${id}`)?.pendingEdit,
    ).toBe(id === 'two' ? 'changed two' : undefined);
  hook.rerender({ enabled: false });
  expect(hook.result.current.items).toEqual([]);
  expect(h.session.journal.queuedInputs.read()).toHaveLength(3);
  expect(h.fetch.mock.calls.filter(([, init]) => init?.method === 'POST')).toEqual([]);
  hook.unmount();
});

it('retains managed source identity after cancellation, reload and access suspension', async () => {
  const h = queueHarness();
  const input = h.session.journal.retain('original one');
  h.session.journal.accepted(input.key, { commandId: 'cmd_one', responseId: 'resp_one' });
  const first = renderHook(() => useQueuedInputs(h.session, true, true, '0', identity));
  await waitFor(() => expect(first.result.current.items).toHaveLength(3));
  await act(() => first.result.current.remove(first.result.current.items[0]!));
  first.unmount();
  const second = renderHook(
    ({ enabled }) => useQueuedInputs(h.session, enabled, true, '0', identity),
    { initialProps: { enabled: true } },
  );
  await waitFor(() => expect(second.result.current.managedCommandIds.has('cmd_one')).toBe(true));
  await waitFor(() => expect(second.result.current.items).toHaveLength(2));
  expect(second.result.current.outcomes[0]).toMatchObject({
    kind: 'cancelled',
    commandId: 'cmd_one',
  });
  second.rerender({ enabled: false });
  second.rerender({ enabled: true });
  await waitFor(() => expect(second.result.current.managedCommandIds.has('cmd_one')).toBe(true));
  expect(second.result.current.items.some((x) => x.commandId === 'cmd_one')).toBe(false);
  second.unmount();
});

it('does not resurrect an interruption message cancelled before this browser observed the queue', async () => {
  const retained = journal();
  retained.interruption.retain({
    key: 'original-interrupt',
    responseId: 'resp_old',
    receipt: null,
    body: { text: 'New message', turn_sequence: 1, expected_agent_revision: 10 },
  });
  retained.interruption.accepted('original-interrupt', {
    controlId: 'cmd_cancel_old',
    commandId: 'cmd_new',
    responseId: 'resp_new',
  });
  const fetch = vi.fn<typeof globalThis.fetch>(async (url) => {
    if (String(url).endsWith('/queued-inputs'))
      return json({
        object: 'cma.agent.queued_inputs',
        agent_id: 'agt_1',
        data: [],
        next_after: null,
      });
    if (String(url).endsWith('/commands/cmd_new')) {
      const status = commandStatus('resp_new');
      return json({
        ...status,
        receipt: { ...status.receipt, command_id: 'cmd_new' },
        status: 'cancelled',
        turn_sequence: null,
        observed_cursor: null,
        delivery: null,
      });
    }
    throw new Error('Unexpected request');
  });
  const session = new AgentV2Session(createClient({ fetch }), retained, 'alice', {
    storage: sessionStorage,
    currentUser: async () => 'alice',
  });
  const first = renderHook(() => useQueuedInputs(session, true, false, '0', identity));
  await waitFor(() => expect(first.result.current.managedCommandIds.has('cmd_new')).toBe(true));
  expect(first.result.current.items).toEqual([]);
  first.unmount();
  const second = renderHook(() => useQueuedInputs(session, true, false, '0', identity));
  await waitFor(() => expect(second.result.current.managedCommandIds.has('cmd_new')).toBe(true));
  expect(retained.queuedInputs.managedCommands().has('cmd_new')).toBe(true);
  expect(fetch.mock.calls.filter(([, init]) => init?.method === 'POST')).toHaveLength(0);
  second.unmount();
});
