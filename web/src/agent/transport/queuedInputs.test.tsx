import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
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
  clearAgentTransport(sessionStorage);
  const second = agentTransportJournal(sessionStorage, 'bob', 'agt_1');
  second.select(true);
  expect(() => first.queuedInputs.settle(intent('one'))).toThrow('sign-in changed');
  expect(second.queuedInputs.read()).toEqual([]);
});

function queueHarness() {
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
  const completedEdits = new Map<string, unknown>();
  const fetch = vi.fn<typeof globalThis.fetch>(async (url, init) => {
    if (String(url).endsWith('/queued-inputs'))
      return json({
        object: 'cma.agent.queued_inputs',
        agent_id: 'agt_1',
        data: rows,
        next_after: null,
      });
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
  await act(() => first.result.current.remove(first.result.current.items[1]!));
  expect(first.result.current.items.map((x) => x.commandId)).toEqual(['cmd_one', 'cmd_three']);
  first.unmount();
  const second = renderHook(() => useQueuedInputs(h.session, true, true, '0', identity));
  await waitFor(() => expect(second.result.current.items).toHaveLength(2));
  await act(() => second.result.current.recover(second.result.current.items[0]!));
  const edits = h.fetch.mock.calls.filter(([url]) => String(url).endsWith('/queued-input'));
  expect(edits).toHaveLength(2);
  expect(edits[1]?.[1]?.body).toBe(edits[0]?.[1]?.body);
  expect(new Headers(edits[1]?.[1]?.headers).get('idempotency-key')).toBe(
    new Headers(edits[0]?.[1]?.headers).get('idempotency-key'),
  );
  expect(second.result.current.items[0]?.needsRecovery).toBe(false);
  expect(h.rows().map((x) => x.text)).toEqual(['revised one', 'original three']);
  second.unmount();
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
  second.rerender({ enabled: false });
  second.rerender({ enabled: true });
  expect(second.result.current.managedCommandIds.has('cmd_one')).toBe(true);
  expect(second.result.current.items.some((x) => x.commandId === 'cmd_one')).toBe(false);
  second.unmount();
});
