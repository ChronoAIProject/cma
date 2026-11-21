import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, expect, it, vi } from 'vitest';
import { useAgentController } from './controller';
import {
  binding,
  cursor,
  harness,
  incarnation,
  item,
  json,
  page,
  queuedPage,
} from './transport/controller.fixture';

const start = item(1, { type: 'turn_started', turn_sequence: 1 }, binding);
const oldApproval = item(2, {
  type: 'approval_requested',
  approval_id: 'old',
  action: 'Old approval',
  detail: null,
});
const failure = (message: string, status = 503) =>
  json(
    {
      error: {
        type: 'server_error',
        code: status === 503 ? 'agent_read_unavailable' : 'response_control_conflict',
        message,
        param: null,
      },
    },
    status,
  );
function opening(active = true) {
  const h = harness();
  h.head.mockImplementation(async () =>
    json({
      object: 'cma.agent.event_head',
      schema_version: 1,
      agent_id: 'agt_1',
      incarnation,
      through_cursor: cursor(2),
      active_turn: active ? start : null,
    }),
  );
  let finish!: (value: Response) => void;
  const held = new Promise<Response>((resolve) => {
    finish = resolve;
  });
  h.snapshot.mockResolvedValueOnce(json(page([start], 2, 1))).mockReturnValueOnce(held);
  return Object.assign(h, { finish: () => finish(json(page([oldApproval], 2))) });
}
beforeEach(() => sessionStorage.clear());

it('uses the live target for Stop while later history is held, without replaying it on merge', async () => {
  const h = opening();
  const view = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(h.streaming).toBe(true));
  await waitFor(() => {
    expect(view.result.current.inputAvailability.allowed).toBe(true);
    expect(view.result.current.responseControls).toBe('available');
    expect(view.result.current.state).toMatchObject({
      historyLoading: true,
      conversation: {
        activeRunId: '1',
        responseBinding: { responseId: 'resp_1', commandId: 'cmd_1', turnSequence: 1 },
      },
    });
  });
  await act(() => view.result.current.interrupt());
  expect(h.control).toHaveBeenCalledOnce();
  expect(h.control.mock.calls[0]![0]).toBe('resp_1');
  expect(JSON.parse(String(h.control.mock.calls[0]![1]!.body))).toMatchObject({
    turn_sequence: 1,
    command: { type: 'cancel' },
  });
  await act(async () => h.durable(3, { type: 'turn_cancelled', turn_sequence: 1 }));
  await waitFor(() =>
    expect(view.result.current.state).toMatchObject({
      historyLoading: true,
      conversation: { activeRunId: undefined, responseBinding: undefined },
    }),
  );
  await act(async () => h.finish());
  await waitFor(() => expect(view.result.current.state).toMatchObject({ historyLoading: false }));
  expect(view.result.current.state).toMatchObject({
    conversation: { activeRunId: undefined, responseBinding: undefined },
  });
  expect(h.control).toHaveBeenCalledOnce();
  view.unmount();
});

it('exposes only live approvals during hydration and never resurrects a resolved approval', async () => {
  const h = opening();
  const view = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledTimes(2));
  await waitFor(() => expect(h.streaming).toBe(true));
  await waitFor(() => expect(view.result.current.inputAvailability.allowed).toBe(true));
  expect(view.result.current.state).toMatchObject({ conversation: { approvals: [] } });
  await act(() => view.result.current.approve('old', 'allow'));
  expect(h.control).not.toHaveBeenCalled();
  await act(async () =>
    h.durable(3, {
      type: 'approval_requested',
      approval_id: 'live',
      action: 'New approval',
      detail: null,
    }),
  );
  await waitFor(() =>
    expect(view.result.current.state).toMatchObject({
      conversation: { approvals: [{ id: 'live' }] },
    }),
  );
  await act(() => view.result.current.approve('live', 'deny'));
  expect(h.control).toHaveBeenCalledOnce();
  expect(JSON.parse(String(h.control.mock.calls[0]![1]!.body)).command).toEqual({
    type: 'resolve_approval',
    approval_id: 'live',
    decision: 'deny',
  });
  await act(async () =>
    h.durable(4, { type: 'approval_resolved', approval_id: 'live', decision: 'deny' }),
  );
  await act(async () => h.finish());
  await waitFor(() => expect(view.result.current.state).toMatchObject({ historyLoading: false }));
  expect(view.result.current.state).toMatchObject({
    conversation: {
      approvals: [{ id: 'old' }, { id: 'live', decision: 'deny' }],
      responseBinding: { responseId: 'resp_1', commandId: 'cmd_1', turnSequence: 1 },
    },
  });
  const state = view.result.current.state;
  if (state.status !== 'ready') throw new Error('Expected hydrated Agent');
  expect(
    state.conversation.approvals
      .filter((request) => request.decision === undefined)
      .map((request) => request.id),
  ).toEqual(['old']);
  expect(h.control).toHaveBeenCalledOnce();
  view.unmount();
});

it.each([true, false])(
  'discovers and cancels a queued input before history finishes (active: %s)',
  async (active) => {
    const h = opening(active);
    h.listQueued.mockImplementation(async () => json(queuedPage()));
    const view = renderHook(() => useAgentController(h.client, 'agt_1'));
    await waitFor(() => expect(view.result.current.queuedInputs?.items).toHaveLength(1));
    expect(view.result.current.state).toMatchObject({ historyLoading: true });
    const row = view.result.current.queuedInputs!.items[0]!;
    h.listQueued.mockImplementation(async () =>
      json({ object: 'cma.agent.queued_inputs', agent_id: 'agt_1', data: [], next_after: null }),
    );
    await act(() => view.result.current.queuedInputs!.remove(row));
    expect(h.cancelQueued).toHaveBeenCalledOnce();
    await act(async () => h.finish());
    await waitFor(() => expect(view.result.current.state).toMatchObject({ historyLoading: false }));
    expect(view.result.current.queuedInputs!.items).toEqual([]);
    expect(h.cancelQueued).toHaveBeenCalledOnce();
    view.unmount();
  },
);

it.each(['stop', 'delete'] as const)(
  'allows %s with a known idle head while history is held',
  async (command) => {
    const h = opening(false);
    const operation = vi
      .spyOn(h.client, command === 'stop' ? 'stopAgent' : 'deleteAgent')
      .mockResolvedValue(undefined);
    const view = renderHook(() => useAgentController(h.client, 'agt_1'));
    await waitFor(() => expect(h.snapshot).toHaveBeenCalledTimes(2));
    await act(() => view.result.current[command]());
    expect(operation).toHaveBeenCalledOnce();
    view.unmount();
  },
);

it('keeps controls usable after a history-page failure while surfacing server refusal once', async () => {
  const h = opening();
  h.snapshot
    .mockReset()
    .mockResolvedValueOnce(json(page([start], 2, 1)))
    .mockResolvedValueOnce(failure('History unavailable'));
  h.control.mockResolvedValueOnce(failure('Target changed', 409));
  const view = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() =>
    expect(view.result.current.state).toMatchObject({ historyUnavailable: true }),
  );
  await act(() => view.result.current.interrupt());
  expect(h.control).toHaveBeenCalledOnce();
  expect(view.result.current.actionError).toBeTruthy();
  expect(view.result.current.controlBaselineReady).toBe(true);
  view.unmount();
});

it('holds all controls when head fails until the complete history supplies a baseline', async () => {
  const h = opening();
  h.head.mockResolvedValue(failure('Head unavailable'));
  const stop = vi.spyOn(h.client, 'stopAgent');
  const view = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(h.snapshot).toHaveBeenCalledTimes(2));
  for (const command of ['interrupt', 'stop', 'delete'] as const)
    await act(() => view.result.current[command]());
  await act(() => view.result.current.approve('old', 'allow'));
  expect(h.control).not.toHaveBeenCalled();
  expect(stop).not.toHaveBeenCalled();
  expect(h.listQueued).not.toHaveBeenCalled();
  await act(async () => h.finish());
  await waitFor(() => expect(view.result.current.controlBaselineReady).toBe(true));
  view.unmount();
});

it('revokes captured callbacks during access review', async () => {
  const h = opening();
  const view = renderHook(({ suspended }) => useAgentController(h.client, 'agt_1', { suspended }), {
    initialProps: { suspended: false },
  });
  await waitFor(() => expect(view.result.current.controlBaselineReady).toBe(true));
  const interrupt = view.result.current.interrupt;
  view.rerender({ suspended: true });
  expect(view.result.current.controlBaselineReady).toBe(false);
  await act(() => interrupt());
  await act(() => view.result.current.interrupt());
  expect(h.control).not.toHaveBeenCalled();
  view.unmount();
});

it('rejects a changed account before submitting a command during hydration', async () => {
  const h = opening();
  const stop = vi.spyOn(h.client, 'stopAgent');
  const view = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(view.result.current.controlBaselineReady).toBe(true));
  await waitFor(() => expect(view.result.current.inputAvailability.allowed).toBe(true));
  await waitFor(() => expect(h.streaming).toBe(true));
  h.currentUser.mockResolvedValue('bob');
  await act(() => view.result.current.stop());
  expect(view.result.current.state).toMatchObject({ status: 'error' });
  expect(view.result.current.controlBaselineReady).toBe(false);
  expect(h.control).not.toHaveBeenCalled();
  expect(stop).not.toHaveBeenCalled();
  view.unmount();
});
