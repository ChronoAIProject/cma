import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, expect, it } from 'vitest';
import { useAgentController } from '../controller';
import { binding, harness, item, json, page } from './controller.fixture';
import { queuedObservation } from './queued.fixture';

beforeEach(() => sessionStorage.clear());
const problem = (code: string) =>
  json(
    {
      error: {
        type: 'invalid_request_error',
        code,
        message: code,
        param: null,
      },
    },
    409,
  );

function queuedHarness() {
  const h = harness();
  h.observeQueued.mockImplementation(async (id) => json(queuedObservation(id)));
  return h;
}
async function open(h: ReturnType<typeof harness>) {
  const view = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(view.result.current.inputAvailability.allowed).toBe(true));
  await act(() => view.result.current.send('Cancel this waiting message'));
  await waitFor(() => expect(view.result.current.queuedMessage?.canCancel).toBe(true));
  return view;
}

it('cancels only the queued source while an independent response stays active', async () => {
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
  await act(() => view.result.current.queuedMessage!.cancel());
  expect(h.cancelQueued).toHaveBeenCalledOnce();
  expect(JSON.parse(String(h.cancelQueued.mock.calls[0]![1]!.body))).toEqual({
    source_command_id: 'cmd_1',
    expected_command_revision: 7,
  });
  expect(view.result.current.queuedMessage?.notice).toContain('cancelled before it started');
  expect(view.result.current.state).toMatchObject({
    conversation: { activeRunId: '1', responseBinding: { responseId: 'resp_active' } },
  });
  expect(h.control).not.toHaveBeenCalled();
  expect(h.client.interruptAgent).not.toHaveBeenCalled();
  expect(h.cancel).not.toHaveBeenCalled();
  view.unmount();
});

it.each(['lost', 'queued_response_cancellation_conflict'])(
  'retains %s and recovers after reload without observing a new cancellation target',
  async (failure) => {
    const h = queuedHarness();
    if (failure === 'lost') h.cancelQueued.mockRejectedValueOnce(new TypeError('lost receipt'));
    else h.cancelQueued.mockResolvedValueOnce(problem(failure));
    const first = await open(h);
    await act(() => first.result.current.queuedMessage!.cancel());
    expect(first.result.current.queuedMessage?.needsRecovery).toBe(true);
    first.unmount();
    h.observeQueued.mockImplementation(async (id) => json(queuedObservation(id, 99, false)));
    const second = renderHook(() => useAgentController(h.client, 'agt_1'));
    await waitFor(() => expect(second.result.current.queuedMessage?.needsRecovery).toBe(true));
    await act(() => second.result.current.queuedMessage!.recover());
    expect(second.result.current.queuedMessage?.needsRecovery).toBe(false);
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

it('clears only definitive absence and requires a new explicit cancellation with fresh revision and key', async () => {
  const h = queuedHarness();
  h.cancelQueued.mockResolvedValueOnce(problem('queued_response_cancellation_not_admitted'));
  const view = await open(h);
  await act(() => view.result.current.queuedMessage!.cancel());
  expect(view.result.current.queuedMessage?.needsRecovery).toBe(false);
  h.observeQueued.mockImplementation(async (id) => json(queuedObservation(id, 8)));
  await waitFor(() => expect(view.result.current.queuedMessage?.canCancel).toBe(true));
  await act(() => view.result.current.queuedMessage!.cancel());
  const original = h.cancelQueued.mock.calls[0]![1]!;
  const next = h.cancelQueued.mock.calls[1]![1]!;
  expect(JSON.parse(String(next.body)).expected_command_revision).toBe(8);
  expect(new Headers(next.headers).get('idempotency-key')).not.toBe(
    new Headers(original.headers).get('idempotency-key'),
  );
  view.unmount();
});

it('refuses cancellation after dispatch without falling back to an active-turn control', async () => {
  const h = queuedHarness();
  const view = await open(h);
  h.observeQueued.mockImplementation(async (id) => json(queuedObservation(id, 8, false)));
  await act(() => view.result.current.queuedMessage!.cancel());
  expect(view.result.current.queuedMessage?.error).toContain('can no longer be cancelled');
  expect(h.cancelQueued).not.toHaveBeenCalled();
  expect(h.control).not.toHaveBeenCalled();
  view.unmount();
});

it('checks identity after observation before admitting a cancellation', async () => {
  const h = queuedHarness();
  const view = await open(h);
  h.observeQueued.mockImplementationOnce(async (id) => {
    h.currentUser.mockResolvedValue('bob');
    return json(queuedObservation(id));
  });
  await act(() => view.result.current.queuedMessage!.cancel());
  expect(view.result.current.state.status).toBe('error');
  expect(h.cancelQueued).not.toHaveBeenCalled();
  expect(Object.keys(sessionStorage).some((key) => key.endsWith(':queued-cancellation:v1'))).toBe(
    false,
  );
  view.unmount();
});
