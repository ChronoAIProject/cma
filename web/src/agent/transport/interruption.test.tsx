import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useAgentController } from '../controller';
import { binding, harness, item, json, page } from './controller.fixture';
import { agentTransportJournal } from './journal';
import { commandStatus, observation } from './controls.fixture';

beforeEach(() => sessionStorage.clear());
afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});
function activeHarness(storage?: Storage) {
  const h = harness(storage);
  h.snapshot.mockImplementation(async () =>
    json(page([item(1, { type: 'turn_started', turn_sequence: 1 }, binding)], 1)),
  );
  return h;
}
function retainedStorage(setItem: Storage['setItem']): Storage {
  return {
    getItem: (key) => sessionStorage.getItem(key),
    setItem,
    removeItem: (key) => sessionStorage.removeItem(key),
    key: (index) => sessionStorage.key(index),
    clear: () => sessionStorage.clear(),
    get length() {
      return sessionStorage.length;
    },
  };
}

it('recovers the exact old target and new message after reload while another turn is active', async () => {
  const h = activeHarness();
  h.interrupt.mockRejectedValueOnce(new TypeError('lost reply'));
  const first = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(first.result.current.inputAvailability.allowed).toBe(true));
  await act(() =>
    first.result.current.interruptWithMessage!('  New turn\nwith attachment reference  '),
  );
  expect(first.result.current.pendingControl).toBe(true);
  first.unmount();
  h.snapshot.mockImplementation(async () =>
    json(
      page(
        [
          item(1, { type: 'turn_started', turn_sequence: 1 }, binding),
          item(2, { type: 'turn_completed', turn_sequence: 1 }),
          item(
            3,
            { type: 'turn_started', turn_sequence: 3 },
            { response_id: 'resp_later', command_id: 'cmd_later', turn_sequence: 3 },
          ),
        ],
        3,
      ),
    ),
  );
  const second = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(second.result.current.state.status).toBe('ready'));
  await act(() => second.result.current.recoverPending!());
  expect(h.interrupt).toHaveBeenCalledTimes(2);
  const [originalTarget, original] = h.interrupt.mock.calls[0]!;
  const [replayedTarget, replayed] = h.interrupt.mock.calls[1]!;
  expect(originalTarget).toBe('resp_1');
  expect(replayedTarget).toBe(originalTarget);
  expect(replayed!.body).toBe(original!.body);
  expect(new Headers(replayed!.headers).get('idempotency-key')).toBe(
    new Headers(original!.headers).get('idempotency-key'),
  );
  expect(h.observe).toHaveBeenCalledOnce();
  expect(second.result.current.pendingControl).toBe(false);
  expect(second.result.current.acceptedInterruptionMessage).toBe(
    '  New turn\nwith attachment reference  ',
  );
  expect(second.result.current.state).toMatchObject({
    conversation: { activeRunId: '3', responseBinding: { responseId: 'resp_later' } },
  });
  expect(h.control).not.toHaveBeenCalled();
  expect(h.post).not.toHaveBeenCalled();
  expect(h.client.interruptAgent).not.toHaveBeenCalled();
  second.unmount();
});

it.each([
  { code: 'response_control_conflict', status: 409, retained: true },
  { code: 'invalid_interrupt_request', status: 422, retained: false },
  { code: 'capability_unsupported', status: 422, retained: false },
])(
  'keeps uncertainty and releases only a definitive no-admission refusal: $code',
  async ({ code, status, retained }) => {
    const h = activeHarness();
    h.interrupt.mockResolvedValueOnce(
      json(
        {
          error: {
            type: 'invalid_request_error',
            code,
            message: 'The original target changed.',
            param: null,
          },
        },
        status,
      ),
    );
    const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
    await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
    const refused = vi.fn();
    await act(() =>
      result.current.interruptWithMessage!('Keep my input', undefined, undefined, refused),
    );
    expect(result.current.pendingControl).toBe(retained);
    expect(refused).toHaveBeenCalledTimes(retained ? 0 : 1);
    if (retained) {
      await act(() => result.current.recoverPending!());
      expect(h.interrupt.mock.calls[1]![1]!.body).toBe(h.interrupt.mock.calls[0]![1]!.body);
    }
    expect(h.post).not.toHaveBeenCalled();
    unmount();
  },
);

it('re-observes once and retries a stale interruption revision while the same turn is active', async () => {
  const h = activeHarness();
  h.observe
    .mockResolvedValueOnce(json(observation('resp_1', 1, 10)))
    .mockResolvedValueOnce(json(observation('resp_1', 1, 12)));
  h.interrupt.mockResolvedValueOnce(
    json(
      {
        error: {
          type: 'invalid_request_error',
          code: 'response_interrupt_not_admitted',
          message: 'The observed Agent revision is stale.',
          param: null,
        },
      },
      409,
    ),
  );
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
  const accepted = vi.fn();
  await act(() => result.current.interruptWithMessage!('Continue in a new turn', accepted));
  expect(result.current.actionError).toBeUndefined();
  expect(result.current.pendingControl).toBe(false);
  expect(accepted).toHaveBeenCalledWith({
    commandId: 'cmd_interrupted_next',
    responseId: 'resp_interrupted_next',
  });
  expect(h.interrupt).toHaveBeenCalledTimes(2);
  const first = h.interrupt.mock.calls[0]![1]!;
  const second = h.interrupt.mock.calls[1]![1]!;
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
  expect(result.current.acceptedInterruptionMessage).toBe('Continue in a new turn');
  unmount();
});

it('does not submit an interruption if the exact intent cannot be retained', async () => {
  const h = activeHarness(
    retainedStorage((key, value) => {
      if (key.includes(':interruption:')) throw new Error('Storage unavailable');
      sessionStorage.setItem(key, value);
    }),
  );
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
  const refused = vi.fn();
  await act(() =>
    result.current.interruptWithMessage!('Retain first', undefined, undefined, refused),
  );
  expect(h.interrupt).not.toHaveBeenCalled();
  expect(refused).toHaveBeenCalledOnce();
  expect(result.current.actionError).toContain('Storage unavailable');
  unmount();
});

it('recovers the same admitted pair if retaining its receipt fails', async () => {
  let fail = true;
  const h = activeHarness(
    retainedStorage((key, value) => {
      if (fail && key.includes(':interruption:') && value.includes('"controlId"')) {
        fail = false;
        throw new Error('Receipt storage failed');
      }
      sessionStorage.setItem(key, value);
    }),
  );
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
  const accepted = vi.fn();
  await act(() => result.current.interruptWithMessage!('Original new input', accepted));
  expect(accepted).not.toHaveBeenCalled();
  expect(result.current.pendingControl).toBe(true);
  await act(() => result.current.recoverPending!());
  expect(h.interrupt).toHaveBeenCalledTimes(2);
  expect(h.interrupt.mock.calls[1]![1]!.body).toBe(h.interrupt.mock.calls[0]![1]!.body);
  expect(result.current.acceptedInterruptionMessage).toBe('Original new input');
  unmount();
});

it.each(['failed', 'cancelled'] as const)(
  'shows a %s cancellation separately from the admitted new message without another mutation',
  async (status) => {
    const h = activeHarness();
    h.command.mockResolvedValueOnce(
      json({
        ...commandStatus(null, 1),
        status,
        failure: status === 'failed' ? 'runtime_lost' : null,
        delivery: status === 'failed' ? 'runtime_lost' : 'execution_observed',
      }),
    );
    const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
    await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
    const accepted = vi.fn();
    await act(() => result.current.interruptWithMessage!('Continue in a new turn', accepted));
    await waitFor(() => expect(result.current.controlNotice).toContain('could not be stopped'));
    expect(result.current.controlNotice).toContain('Your new message is saved');
    expect(accepted).toHaveBeenCalledWith({
      commandId: 'cmd_interrupted_next',
      responseId: 'resp_interrupted_next',
    });
    expect(result.current.acceptedInterruptionMessage).toBe('Continue in a new turn');
    expect(result.current.state).toMatchObject({ conversation: { activeRunId: '1' } });
    expect(h.interrupt).toHaveBeenCalledOnce();
    expect(h.command).toHaveBeenCalledOnce();
    expect(h.control).not.toHaveBeenCalled();
    expect(h.post).not.toHaveBeenCalled();
    unmount();
  },
);

it('hides successful interruption delivery without declaring the old turn completed', async () => {
  const h = activeHarness();
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
  await act(() => result.current.interruptWithMessage!('New turn'));
  await waitFor(() => expect(h.command).toHaveBeenCalledOnce());
  expect(result.current.controlNotice).toBeUndefined();
  expect(result.current.acceptedInterruptionMessage).toBe('New turn');
  expect(result.current.state).toMatchObject({ conversation: { activeRunId: '1' } });
  expect(h.interrupt).toHaveBeenCalledOnce();
  unmount();
});

it('sends ordinary input when the original target is conclusively inactive before any mutation', async () => {
  const h = activeHarness();
  h.observe.mockResolvedValueOnce(json({ ...observation(), target_active: false }));
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
  const ordinary = vi.fn();
  const accepted = vi.fn();
  await act(() =>
    result.current.interruptWithMessage!(
      'Keep this new message',
      accepted,
      undefined,
      undefined,
      ordinary,
    ),
  );
  expect(ordinary).toHaveBeenCalledOnce();
  expect(accepted).toHaveBeenCalledOnce();
  expect(h.interrupt).not.toHaveBeenCalled();
  expect(h.control).not.toHaveBeenCalled();
  expect(h.post).toHaveBeenCalledOnce();
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
  expect(journal.interruption.read()).toBeNull();
  expect(journal.read()).toMatchObject({ state: 'accepted', input: 'Keep this new message' });
  unmount();
});

it('never turns a mismatched inactive observation into ordinary input or a new cancellation target', async () => {
  const h = activeHarness();
  h.observe.mockResolvedValueOnce(json({ ...observation('resp_1', 3), target_active: false }));
  const { result, unmount } = renderHook(() => useAgentController(h.client, 'agt_1'));
  await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
  const ordinary = vi.fn();
  await act(() =>
    result.current.interruptWithMessage!(
      'Keep my draft',
      undefined,
      undefined,
      undefined,
      ordinary,
    ),
  );
  expect(ordinary).not.toHaveBeenCalled();
  expect(h.post).not.toHaveBeenCalled();
  expect(h.interrupt).not.toHaveBeenCalled();
  expect(result.current.actionError).toContain('response target changed');
  unmount();
});

it('fences pending interruptions across ordinary input, other actions and account changes', () => {
  const journal = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
  journal.select(true);
  const intent = {
    key: 'original',
    responseId: 'resp_1',
    body: { turn_sequence: 1, expected_agent_revision: 10, text: 'Original' },
    receipt: null,
  } as const;
  journal.interruption.retain(intent);
  expect(() => journal.retain('Another')).toThrow('pending interruption');
  expect(() =>
    journal.retainControl({
      key: 'cancel',
      responseId: 'resp_1',
      commandId: null,
      body: { turn_sequence: 1, expected_agent_revision: 10, command: { type: 'cancel' } },
    }),
  ).toThrow('pending interruption');
  expect(() =>
    journal.interruption.retain({ ...intent, body: { ...intent.body, text: 'Changed' } }),
  ).toThrow('pending interruption');
  expect(journal.interruption.retain({ ...intent, key: 'different-local-key' }).key).toBe(
    'original',
  );
  const other = agentTransportJournal(sessionStorage, 'bob', 'agt_1');
  other.select(true);
  expect(() => journal.interruption.read()).toThrow();
  expect(other.interruption.read()).toBeNull();
});
