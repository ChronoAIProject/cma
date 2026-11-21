import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError, TransportError } from '@cma/client';
import { accepted, harness, json } from './controller.fixture';
import { commandStatus } from './controls.fixture';
import { recoverV2Control, submitV2Control } from './controls';
import { agentTransportJournal, clearAgentTransport } from './journal';
import { isIdentityChanged } from './identity';

beforeEach(() => {
  sessionStorage.clear();
  vi.useFakeTimers();
});
afterEach(() => vi.useRealTimers());
const target = { responseId: 'resp_1', commandId: 'cmd_1', turnSequence: 1 };
const errorResponse = (status: number, code = 'temporary') =>
  json(
    {
      error: {
        type: 'server_error',
        code,
        message: 'Request unavailable',
        param: null,
      },
    },
    status,
  );
async function setup(enabled = true) {
  const h = harness(sessionStorage, { enabled, delay: () => 100 });
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  h.post.mockImplementation(async () => accepted());
  return { ...h, session };
}
const key = (init?: RequestInit) => new Headers(init?.headers).get('idempotency-key');
const cancel = (h: Awaited<ReturnType<typeof setup>>, signal = new AbortController().signal) =>
  submitV2Control(h.session, target, { type: 'cancel' }, 'original-key', signal);

it('recovers a lost Send receipt with identical input, answer context and key', async () => {
  const h = await setup();
  h.post.mockRejectedValueOnce(new TypeError('ingress lost'));
  h.inputReceipt.mockImplementationOnce(async () => json(commandStatus('resp_1', 1, true)));
  const onAccepted = vi.fn();
  const send = h.session.send('original\ninput', onAccepted, undefined, undefined, {
    turn_sequence: 1,
    answer_sequence: 2,
  });
  await vi.runAllTimersAsync();
  await send;
  expect(h.post).toHaveBeenCalledOnce();
  expect(h.inputReceipt).toHaveBeenCalledOnce();
  const first = h.post.mock.lastCall?.[0];
  const second = h.inputReceipt.mock.lastCall?.[0];
  expect(key(first)).toBeTruthy();
  expect(key(second)).toBe(key(first));
  expect(second?.body).toBe(first?.body);
  expect(JSON.parse(String(second?.body)).answer_context).toEqual({
    turn_sequence: 1,
    answer_sequence: 2,
  });
  expect(onAccepted).toHaveBeenCalledOnce();
  expect(h.session.journal.read()?.state).toBe('accepted');
});

it.each(['send', 'cancel'] as const)(
  'retries fresh %s receipt confirmation without another POST',
  async (action) => {
    const h = await setup();
    h.currentUser.mockResolvedValueOnce('alice');
    if (action === 'cancel') h.currentUser.mockResolvedValueOnce('alice');
    h.currentUser.mockRejectedValueOnce(new TransportError('authority temporarily unavailable'));
    const onAccepted = vi.fn();
    const operation = action === 'send' ? h.session.send('input', onAccepted) : cancel(h);
    await vi.runAllTimersAsync();
    await operation;
    expect(action === 'send' ? h.post : h.control).toHaveBeenCalledOnce();
    if (action === 'send') expect(onAccepted).toHaveBeenCalledOnce();
    else expect(h.session.journal.readControl()?.commandId).toBe('cmd_control');
  },
);

it('replays Cancel without observing or retargeting after ambiguous loss', async () => {
  const h = await setup();
  h.control.mockRejectedValueOnce(new TypeError('receipt lost'));
  const operation = cancel(h);
  await vi.runAllTimersAsync();
  await operation;
  expect(h.observe).toHaveBeenCalledOnce();
  expect(h.control).toHaveBeenCalledTimes(2);
  const [first, second] = h.control.mock.calls;
  expect(first![0]).toBe(second![0]);
  expect(first![1]?.body).toBe(second![1]?.body);
  expect(key(second![1])).toBe('original-key');
  expect(h.session.journal.readControl()?.body).toEqual({
    turn_sequence: 1,
    expected_agent_revision: 10,
    command: { type: 'cancel' },
  });
});

it.each([401, 403, 409])(
  'preserves uncertain Send and Cancel on %s without automatic retry',
  async (status) => {
    const h = await setup();
    h.post.mockImplementationOnce(async () => errorResponse(status));
    h.control.mockImplementationOnce(async () => errorResponse(status));
    await expect(h.session.send('input', vi.fn())).rejects.toBeInstanceOf(ApiError);
    await expect(cancel(h)).rejects.toBeInstanceOf(ApiError);
    expect(h.post).toHaveBeenCalledOnce();
    expect(h.control).toHaveBeenCalledOnce();
    expect(h.session.journal.read()?.state).toBe('pending');
    expect(h.session.journal.readControl()?.commandId).toBeNull();
  },
);

it('does not turn a refusal after ambiguous retry into an automatic fresh-key request', async () => {
  const h = await setup();
  h.control
    .mockRejectedValueOnce(new TypeError('lost'))
    .mockImplementationOnce(async () => errorResponse(409, 'response_control_not_admitted'));
  const operation = expect(cancel(h)).rejects.toBeInstanceOf(ApiError);
  await vi.runAllTimersAsync();
  await operation;
  expect(h.observe).toHaveBeenCalledOnce();
  expect(h.control).toHaveBeenCalledTimes(2);
  expect(h.control.mock.calls.map(([, init]) => key(init))).toEqual([
    'original-key',
    'original-key',
  ]);
  expect(h.session.journal.readControl()).toBeNull();
});

it.each(['steer', 'resolve_approval'] as const)(
  'leaves %s and retained recovery outside automatic mutation retry',
  async (type) => {
    const h = await setup();
    const command =
      type === 'steer'
        ? { type, text: 'steering' }
        : { type, approval_id: 'approval', decision: 'allow' as const };
    h.control.mockRejectedValue(new TypeError('lost'));
    await expect(
      submitV2Control(h.session, target, command, 'key', new AbortController().signal),
    ).rejects.toThrow();
    await expect(recoverV2Control(h.session, new AbortController().signal)).rejects.toThrow();
    expect(h.control).toHaveBeenCalledTimes(2);
    expect(h.observe).toHaveBeenCalledOnce();
  },
);

it.each(['send', 'cancel'] as const)(
  'stops %s retry when navigation aborts during sleep',
  async (action) => {
    const h = await setup();
    const controller = new AbortController();
    h.post.mockRejectedValueOnce(new TypeError('lost'));
    h.control.mockRejectedValueOnce(new TypeError('lost'));
    const result = (
      action === 'send'
        ? h.session.send('input', vi.fn(), controller.signal)
        : cancel(h, controller.signal)
    ).catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(0);
    controller.abort();
    await vi.runAllTimersAsync();
    expect(await result).toMatchObject({ name: 'AbortError' });
    expect(action === 'send' ? h.post : h.control).toHaveBeenCalledOnce();
  },
);

it.each(['account', 'epoch'] as const)(
  'stops exact-intent Send retry after %s changes during sleep',
  async (change) => {
    const h = await setup();
    h.post.mockRejectedValueOnce(new TypeError('lost'));
    const onAccepted = vi.fn();
    const result = h.session.send('input', onAccepted).catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(0);
    if (change === 'account') h.currentUser.mockResolvedValue('bob');
    else {
      clearAgentTransport(sessionStorage);
      agentTransportJournal(sessionStorage, 'alice', 'agt_1').select(true);
    }
    await vi.runAllTimersAsync();
    expect(isIdentityChanged(await result)).toBe(true);
    expect(h.post).toHaveBeenCalledOnce();
    expect(h.inputReceipt).not.toHaveBeenCalled();
    expect(onAccepted).not.toHaveBeenCalled();
  },
);

it('bounds failed receipt reads, retaining the original pending input', async () => {
  const h = await setup();
  h.post.mockRejectedValueOnce(new TypeError('lost'));
  h.inputReceipt.mockRejectedValue(new TypeError('receipt temporarily unavailable'));
  const operation = expect(h.session.send('input', vi.fn())).rejects.toThrow();
  await vi.runAllTimersAsync();
  await operation;
  expect(h.post).toHaveBeenCalledOnce();
  expect(h.inputReceipt).toHaveBeenCalledTimes(4);
  expect(h.inputReceipt.mock.calls.map(([init]) => key(init))).toEqual(
    Array(4).fill(key(h.post.mock.lastCall?.[0])),
  );
  expect(h.session.journal.read()?.state).toBe('pending');
});

it('keeps default-off loss recoverable only through explicit action', async () => {
  const h = await setup(false);
  h.post.mockRejectedValueOnce(new TypeError('lost'));
  await expect(h.session.send('input', vi.fn())).rejects.toThrow();
  expect(h.post).toHaveBeenCalledOnce();
  // Node 24's Response machinery queues zero-delay cleanup tasks. Drain only
  // that turn, then verify recovery scheduled no timer or automatic request.
  await vi.advanceTimersByTimeAsync(0);
  expect(h.post).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
  expect(h.inputReceipt).not.toHaveBeenCalled();
  expect(await h.session.recover(vi.fn())).toBe(false);
  expect(h.post).toHaveBeenCalledOnce();
  expect(h.inputReceipt).toHaveBeenCalledOnce();
  expect(key(h.inputReceipt.mock.lastCall?.[0])).toBe(key(h.post.mock.lastCall?.[0]));
});

it.each(['send', 'cancel'] as const)(
  'does not accept a %s receipt after the retained body changed during fresh confirmation',
  async (action) => {
    const h = await setup();
    h.currentUser.mockResolvedValueOnce('alice');
    if (action === 'cancel') h.currentUser.mockResolvedValueOnce('alice');
    h.currentUser.mockImplementationOnce(async () => {
      const storageKey = Object.keys(sessionStorage).find((key) => key.endsWith(':alice:agt_1'))!;
      const record = JSON.parse(sessionStorage.getItem(storageKey)!);
      if (action === 'send') record.input.input = 'changed';
      else record.control.body.expected_agent_revision = 99;
      sessionStorage.setItem(storageKey, JSON.stringify(record));
      return 'alice';
    });
    const onAccepted = vi.fn();
    const failure = await (
      action === 'send' ? h.session.send('input', onAccepted) : cancel(h)
    ).catch((error: unknown) => error);
    const retainedFailure = action === 'send' ? (failure as Error).cause : failure;
    expect(retainedFailure).toBeInstanceOf(Error);
    expect((retainedFailure as Error).message).toContain('retained');
    expect(onAccepted).not.toHaveBeenCalled();
    expect(action === 'send' ? h.post : h.control).toHaveBeenCalledOnce();
    if (action === 'send') expect(h.session.journal.read()?.state).toBe('pending');
    else expect(h.session.journal.readControl()?.commandId).toBeNull();
  },
);

it.each(['account', 'epoch'] as const)(
  'stops retained Cancel recovery after %s changes during sleep',
  async (change) => {
    const h = await setup();
    h.control.mockRejectedValueOnce(new TypeError('lost'));
    const result = cancel(h).catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(0);
    if (change === 'account') h.currentUser.mockResolvedValue('bob');
    else {
      clearAgentTransport(sessionStorage);
      agentTransportJournal(sessionStorage, 'alice', 'agt_1').select(true);
    }
    await vi.runAllTimersAsync();
    expect(isIdentityChanged(await result)).toBe(true);
    expect(h.control).toHaveBeenCalledOnce();
  },
);

it('does not retry a malformed Cancel receipt or clear its pending intent', async () => {
  const h = await setup();
  h.control.mockResolvedValueOnce(
    new Response('{', { status: 202, headers: { 'content-type': 'application/json' } }),
  );
  await expect(cancel(h)).rejects.toThrow();
  expect(h.control).toHaveBeenCalledOnce();
  expect(h.session.journal.readControl()?.commandId).toBeNull();
});

it.each(['automatic', 'explicit', 'contention'] as const)(
  'retains uncertain Send after an unauthorized receipt read during %s recovery',
  async (mode) => {
    const h = await setup(mode === 'automatic');
    if (mode === 'contention')
      h.post.mockImplementationOnce(async () =>
        errorResponse(409, 'agent_command_admission_conflict'),
      );
    else h.post.mockRejectedValueOnce(new TypeError('lost committed receipt'));
    h.inputReceipt.mockImplementationOnce(async () => errorResponse(403, 'response_forbidden'));
    const onAccepted = vi.fn();
    const operation = expect(h.session.send('original', onAccepted)).rejects.toThrow();
    await vi.runAllTimersAsync();
    await operation;
    if (mode !== 'automatic') await expect(h.session.recover(onAccepted)).rejects.toThrow();
    expect(h.post).toHaveBeenCalledOnce();
    expect(h.inputReceipt).toHaveBeenCalledOnce();
    expect(key(h.inputReceipt.mock.lastCall?.[0])).toBe(key(h.post.mock.lastCall?.[0]));
    expect(h.session.journal.read()).toMatchObject({ state: 'pending', input: 'original' });
    expect(onAccepted).not.toHaveBeenCalled();
  },
);
