import { act, renderHook, waitFor } from '@testing-library/react';
import { beforeEach, expect, it } from 'vitest';
import { useAgentController } from '../controller';
import { binding, harness, item, json, page } from './controller.fixture';
import { commandStatus, observation } from './controls.fixture';

beforeEach(() => sessionStorage.clear());

function activeHarness() {
  const h = harness();
  h.snapshot.mockImplementation(async () =>
    json(page([item(1, { type: 'turn_started', turn_sequence: 1 }, binding)], 1)),
  );
  return h;
}

it('retries a busy Stop response with the original target, body and key', async () => {
  const h = activeHarness();
  h.control.mockResolvedValueOnce(
    json(
      {
        error: {
          type: 'server_error',
          code: 'response_control_busy',
          message: 'Busy',
          param: null,
        },
      },
      503,
    ),
  );
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
  await act(() => result.current.interrupt());
  expect(h.control).toHaveBeenCalledTimes(2);
  const [originalTarget, original] = h.control.mock.calls[0]!;
  const [retryTarget, retry] = h.control.mock.calls[1]!;
  expect(retryTarget).toBe(originalTarget);
  expect(retry?.body).toBe(original?.body);
  expect(new Headers(retry?.headers).get('idempotency-key')).toBe(
    new Headers(original?.headers).get('idempotency-key'),
  );
  expect(h.observe).toHaveBeenCalledOnce();
  expect(result.current.pendingControl).toBe(false);
  unmount();
});

it.each(['cancel', 'approval'] as const)(
  'recovers unknown %s on its original response, revision, turn, body and key after reload',
  async (kind) => {
    const h = activeHarness();
    h.control.mockRejectedValueOnce(new TypeError('lost receipt'));
    const first = renderHook(() => useAgentController(h.client, 'agt_1'));
    await waitFor(() => expect(first.result.current.inputAvailability.allowed).toBe(true));
    await act(() =>
      kind === 'cancel'
        ? first.result.current.interrupt()
        : first.result.current.approve('approval_original', 'deny'),
    );
    expect(first.result.current.pendingControl).toBe(true);
    first.unmount();
    // A later observed turn must never become the target of recovery.
    h.snapshot.mockImplementation(async () =>
      json(
        page(
          [
            item(1, { type: 'turn_started', turn_sequence: 1 }, binding),
            item(2, { type: 'turn_completed', turn_sequence: 1 }),
            item(
              3,
              { type: 'turn_started', turn_sequence: 3 },
              {
                response_id: 'resp_new',
                command_id: 'cmd_new',
                turn_sequence: 3,
              },
            ),
          ],
          3,
        ),
      ),
    );
    const second = renderHook(() => useAgentController(h.client, 'agt_1'));
    await waitFor(() =>
      expect(second.result.current.state).toMatchObject({
        historyLoading: false,
        historyUnavailable: false,
      }),
    );
    await act(() => second.result.current.recoverPending!());
    expect(h.control).toHaveBeenCalledTimes(2);
    const [originalTarget, original] = h.control.mock.calls[0]!;
    const [recoveredTarget, recovered] = h.control.mock.calls[1]!;
    expect(recoveredTarget).toBe(originalTarget);
    expect(originalTarget).toBe('resp_1');
    expect(recovered!.body).toBe(original!.body);
    expect(JSON.parse(String(recovered!.body))).toMatchObject({
      turn_sequence: 1,
      expected_agent_revision: 10,
    });
    expect(new Headers(recovered!.headers).get('idempotency-key')).toBe(
      new Headers(original!.headers).get('idempotency-key'),
    );
    expect(h.observe).toHaveBeenCalledOnce();
    expect(second.result.current.pendingControl).toBe(false);
    expect(second.result.current.state).toMatchObject({
      conversation: { activeRunId: '3', responseBinding: { responseId: 'resp_new' } },
    });
    expect(h.client.interruptAgent).not.toHaveBeenCalled();
    expect(h.client.runAgent).not.toHaveBeenCalled();
    second.unmount();
  },
);

it('re-observes once and retries a stale revision while the same turn is active', async () => {
  const h = activeHarness();
  h.observe
    .mockResolvedValueOnce(json(observation('resp_1', 1, 10)))
    .mockResolvedValueOnce(json(observation('resp_1', 1, 12)));
  h.control.mockResolvedValueOnce(
    json(
      {
        error: {
          type: 'invalid_request_error',
          code: 'response_control_not_admitted',
          message: 'The observed target changed.',
          param: null,
        },
      },
      409,
    ),
  );
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
  await act(() => result.current.interrupt());
  expect(result.current.pendingControl).toBe(false);
  expect(result.current.actionError).toBeUndefined();
  expect(h.control).toHaveBeenCalledTimes(2);
  const first = h.control.mock.calls[0]![1]!;
  const second = h.control.mock.calls[1]![1]!;
  expect(h.control.mock.calls[1]![0]).toBe('resp_1');
  expect(JSON.parse(String(first.body))).toMatchObject({
    turn_sequence: 1,
    expected_agent_revision: 10,
  });
  expect(JSON.parse(String(second.body))).toMatchObject({
    turn_sequence: 1,
    expected_agent_revision: 12,
  });
  expect(new Headers(second.headers).get('idempotency-key')).not.toBe(
    new Headers(first.headers).get('idempotency-key'),
  );
  expect(h.observe).toHaveBeenCalledTimes(2);
  await waitFor(() => expect(result.current.controlNotice).toBeUndefined());
  // Completed means control delivery. The actual response remains active.
  expect(result.current.state).toMatchObject({ conversation: { activeRunId: '1' } });
  unmount();
});

it('reports an accepted control delivery failure from GET without sending another action', async () => {
  const h = activeHarness();
  h.command.mockResolvedValueOnce(
    json({
      ...commandStatus(null, 1),
      status: 'failed',
      failure: 'runtime_lost',
      delivery: 'runtime_lost',
    }),
  );
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
  await act(() => result.current.interrupt());
  await waitFor(() => expect(result.current.controlNotice).toContain('could not be delivered'));
  expect(result.current.state).toMatchObject({ conversation: { activeRunId: '1' } });
  expect(h.control).toHaveBeenCalledOnce();
  expect(h.command).toHaveBeenCalledOnce();
  unmount();
});

it('retains an unknown control when a fresh observation conflict prevents receipt recovery', async () => {
  const h = activeHarness();
  h.control.mockRejectedValueOnce(new TypeError('lost admission receipt')).mockResolvedValueOnce(
    json(
      {
        error: {
          type: 'invalid_request_error',
          code: 'response_control_conflict',
          message: 'Fresh observation changed',
          param: null,
        },
      },
      409,
    ),
  );
  const first = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(first.result.current.inputAvailability.allowed).toBe(true));
  await act(() => first.result.current.interrupt());
  first.unmount();
  const second = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() =>
    expect(second.result.current.state).toMatchObject({
      historyLoading: false,
      historyUnavailable: false,
    }),
  );
  await act(() => second.result.current.recoverPending!());
  expect(second.result.current.pendingControl).toBe(true);
  await act(() => second.result.current.recoverPending!());
  expect(second.result.current.pendingControl).toBe(false);
  expect(h.observe).toHaveBeenCalledOnce();
  expect(h.control).toHaveBeenCalledTimes(3);
  const original = h.control.mock.calls[0]![1]!;
  for (const [target, attempt] of h.control.mock.calls.slice(1)) {
    expect(target).toBe('resp_1');
    expect(attempt!.body).toBe(original.body);
    expect(new Headers(attempt!.headers).get('idempotency-key')).toBe(
      new Headers(original.headers).get('idempotency-key'),
    );
  }
  second.unmount();
});

it('treats a generic conflict on the first JS invocation as uncertain because browsers may retry transport', async () => {
  const h = activeHarness();
  h.control.mockResolvedValueOnce(
    json(
      {
        error: {
          type: 'invalid_request_error',
          code: 'response_control_conflict',
          message: 'Fresh observation changed',
          param: null,
        },
      },
      409,
    ),
  );
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
  await act(() => result.current.interrupt());
  expect(result.current.pendingControl).toBe(true);
  await act(() => result.current.recoverPending!());
  expect(result.current.pendingControl).toBe(false);
  const first = h.control.mock.calls[0]![1]!;
  const second = h.control.mock.calls[1]![1]!;
  expect(second.body).toBe(first.body);
  expect(new Headers(second.headers).get('idempotency-key')).toBe(
    new Headers(first.headers).get('idempotency-key'),
  );
  expect(h.observe).toHaveBeenCalledOnce();
  unmount();
});

it('deduplicates pending Stop and drops availability when access is suspended', async () => {
  const h = activeHarness();
  let release!: (value: Response) => void;
  const accepted = await h.control('resp_1', {
    body: JSON.stringify({
      turn_sequence: 1,
      expected_agent_revision: 10,
      command: { type: 'cancel' },
    }),
  });
  h.control.mockClear();
  h.control.mockImplementationOnce(
    () =>
      new Promise((resolve) => {
        release = resolve;
      }),
  );
  const view = renderHook(({ suspended }) => useAgentController(h.client, 'agt_1', { suspended }), {
    initialProps: { suspended: false },
  });
  await waitFor(() => expect(view.result.current.responseControls).toBe('available'));
  let request!: Promise<void>;
  act(() => {
    request = view.result.current.interrupt();
  });
  await waitFor(() => expect(h.control).toHaveBeenCalledOnce());
  await act(() => view.result.current.interrupt());
  expect(h.control).toHaveBeenCalledOnce();
  await act(async () => {
    release(accepted);
    await request;
  });
  view.rerender({ suspended: true });
  expect(view.result.current.responseControls).toBe('checking-access');
  await act(() => view.result.current.interrupt());
  expect(h.control).toHaveBeenCalledOnce();
  view.unmount();
});
