import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, expect, it } from 'vitest';
import { useAgentController } from '../controller';
import { binding, harness, item, json, page, queuedPage } from './controller.fixture';
import { queuedReceipt } from './queued.fixture';

beforeEach(() => sessionStorage.clear());
const problem = (code: string) =>
  json({ error: { type: 'invalid_request_error', code, message: code, param: null } }, 409);
const emptyQueue = () =>
  json({ object: 'cma.agent.queued_inputs', agent_id: 'agt_1', data: [], next_after: null });

function queuedHarness() {
  const h = harness();
  h.listQueued.mockImplementation(async () => json(queuedPage()));
  h.cancelQueued.mockImplementation(async (id, init) => {
    h.listQueued.mockImplementation(async () => emptyQueue());
    return json(queuedReceipt(id, JSON.parse(String(init?.body))));
  });
  return h;
}
async function open(h: ReturnType<typeof harness>) {
  const view = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(view.result.current.inputAvailability.allowed).toBe(true));
  await act(() => view.result.current.send('Cancel this waiting message'));
  await waitFor(() => expect(view.result.current.queuedInputs?.items).toHaveLength(1));
  return view;
}

it('removes only the queued source while an independent response stays active', async () => {
  const h = queuedHarness();
  h.snapshot.mockImplementation(async () =>
    json(
      page(
        [
          item(
            1,
            { type: 'turn_started', turn_sequence: 1 },
            { ...binding, response_id: 'resp_active', command_id: 'cmd_active' },
          ),
        ],
        1,
      ),
    ),
  );
  const view = await open(h);
  await act(() =>
    view.result.current.queuedInputs!.remove(view.result.current.queuedInputs!.items[0]!),
  );
  expect(h.cancelQueued).toHaveBeenCalledOnce();
  expect(JSON.parse(String(h.cancelQueued.mock.calls[0]![1]!.body))).toEqual({
    source_command_id: 'cmd_1',
    expected_command_revision: 7,
  });
  expect(view.result.current.queuedInputs?.items).toHaveLength(0);
  expect(view.result.current.state).toMatchObject({
    conversation: { activeRunId: '1', responseBinding: { responseId: 'resp_active' } },
  });
  expect(h.control).not.toHaveBeenCalled();
  expect(h.client.interruptAgent).not.toHaveBeenCalled();
  expect(h.cancel).not.toHaveBeenCalled();
  view.unmount();
});

it.each(['lost', 'queued_response_cancellation_conflict'])(
  'retains %s and recovers after reload without selecting a new cancellation target',
  async (failure) => {
    const h = queuedHarness();
    if (failure === 'lost') h.cancelQueued.mockRejectedValueOnce(new TypeError('lost receipt'));
    else h.cancelQueued.mockResolvedValueOnce(problem(failure));
    const first = await open(h);
    await act(async () => {
      await expect(
        first.result.current.queuedInputs!.remove(first.result.current.queuedInputs!.items[0]!),
      ).rejects.toThrow();
    });
    expect(first.result.current.queuedInputs?.items[0]?.needsRecovery).toBe(true);
    first.unmount();
    h.listQueued.mockImplementation(async () => emptyQueue());
    const second = renderHook(() => useAgentController(h.client, 'agt_1'));
    await waitFor(() =>
      expect(second.result.current.queuedInputs?.items[0]?.needsRecovery).toBe(true),
    );
    await act(() =>
      second.result.current.queuedInputs!.recover(second.result.current.queuedInputs!.items[0]!),
    );
    expect(second.result.current.queuedInputs?.items).toHaveLength(0);
    expect(h.cancelQueued).toHaveBeenCalledTimes(2);
    const [target, original] = h.cancelQueued.mock.calls[0]!;
    const [recoveredTarget, recovered] = h.cancelQueued.mock.calls[1]!;
    expect(recoveredTarget).toBe(target);
    expect(recovered!.body).toBe(original!.body);
    expect(new Headers(recovered!.headers).get('idempotency-key')).toBe(
      new Headers(original!.headers).get('idempotency-key'),
    );
    expect(JSON.parse(String(recovered!.body)).expected_command_revision).toBe(7);
    second.unmount();
  },
);

it('clears only definitive absence and requires explicit removal with a refreshed revision and key', async () => {
  const h = queuedHarness();
  h.cancelQueued.mockResolvedValueOnce(problem('queued_response_cancellation_not_admitted'));
  const view = await open(h);
  await act(async () => {
    await expect(
      view.result.current.queuedInputs!.remove(view.result.current.queuedInputs!.items[0]!),
    ).rejects.toThrow();
  });
  expect(view.result.current.queuedInputs?.items[0]?.needsRecovery).toBe(false);
  h.listQueued.mockImplementation(async () => json(queuedPage('Cancel this waiting message', 8)));
  act(() => view.result.current.queuedInputs!.refresh());
  await waitFor(() => expect(view.result.current.queuedInputs?.items[0]?.revision).toBe(8));
  await act(() =>
    view.result.current.queuedInputs!.remove(view.result.current.queuedInputs!.items[0]!),
  );
  const original = h.cancelQueued.mock.calls[0]![1]!;
  const next = h.cancelQueued.mock.calls[1]![1]!;
  expect(JSON.parse(String(next.body)).expected_command_revision).toBe(8);
  expect(new Headers(next.headers).get('idempotency-key')).not.toBe(
    new Headers(original.headers).get('idempotency-key'),
  );
  view.unmount();
});

it('handles dispatch winning a removal race without falling back to an active-turn control', async () => {
  const h = queuedHarness();
  const view = await open(h);
  h.cancelQueued.mockResolvedValueOnce(problem('queued_response_cancellation_not_admitted'));
  h.listQueued.mockImplementation(async () => emptyQueue());
  await act(async () => {
    await expect(
      view.result.current.queuedInputs!.remove(view.result.current.queuedInputs!.items[0]!),
    ).rejects.toThrow('queued_response_cancellation_not_admitted');
  });
  expect(view.result.current.queuedInputs?.items[0]?.needsRecovery).toBe(false);
  await waitFor(() => expect(view.result.current.queuedInputs?.items).toHaveLength(0));
  expect(h.cancelQueued).toHaveBeenCalledOnce();
  expect(h.control).not.toHaveBeenCalled();
  expect(h.client.interruptAgent).not.toHaveBeenCalled();
  view.unmount();
});

it('checks current identity before retaining or sending a queued removal', async () => {
  const h = queuedHarness();
  const view = await open(h);
  h.currentUser.mockResolvedValue('bob');
  await act(async () => {
    await expect(
      view.result.current.queuedInputs!.remove(view.result.current.queuedInputs!.items[0]!),
    ).rejects.toThrow();
  });
  expect(view.result.current.state.status).toBe('error');
  expect(h.cancelQueued).not.toHaveBeenCalled();
  expect(Object.keys(sessionStorage).some((key) => key.includes(':queued-inputs:'))).toBe(false);
  view.unmount();
});
