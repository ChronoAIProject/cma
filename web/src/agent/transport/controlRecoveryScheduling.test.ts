import { beforeEach, expect, it } from 'vitest';
import { TransportError } from '@cma/client';
import { harness, json } from './controller.fixture';
import { recoverV2Control } from './controls';
import { clearAgentTransport } from './journal';
import { IdentityChangedError } from './identity';

beforeEach(() => sessionStorage.clear());

async function retained(retry = false) {
  const h = harness(sessionStorage, { enabled: retry, delay: () => 0 });
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const intent = session.journal.retainControl({
    key: 'original-stop',
    responseId: 'resp_1',
    commandId: null,
    body: { turn_sequence: 1, expected_agent_revision: 10, command: { type: 'cancel' } },
  });
  h.currentUser.mockClear();
  return { ...h, session, intent };
}

it('dispatches retained Stop after one fresh check and separately authorizes its receipt', async () => {
  const h = await retained();
  const order: string[] = [];
  h.currentUser.mockImplementation(async () => {
    order.push('identity');
    return 'alice';
  });
  const submit = h.control.getMockImplementation()!;
  h.control.mockImplementation(async (...args) => {
    order.push('POST');
    return submit(...args);
  });
  await recoverV2Control(h.session, new AbortController().signal);
  expect(order).toEqual(['identity', 'POST', 'identity']);
  expect(h.observe).not.toHaveBeenCalled();
  expect(h.control.mock.calls[0]![0]).toBe(h.intent.responseId);
  const request = h.control.mock.calls[0]![1]!;
  expect(JSON.parse(String(request.body))).toEqual(h.intent.body);
  expect(new Headers(request.headers).get('idempotency-key')).toBe(h.intent.key);
  expect(h.session.journal.readControl()?.commandId).toBe('cmd_control');
});

it.each(['account', 'unavailable', 'abort', 'epoch', 'intent'] as const)(
  'refuses recovery when %s changes during its fresh pre-dispatch check',
  async (change) => {
    const h = await retained();
    const controller = new AbortController();
    h.currentUser.mockImplementationOnce(async () => {
      if (change === 'account') return 'bob';
      if (change === 'unavailable') throw new Error('identity unavailable');
      if (change === 'abort') controller.abort();
      if (change === 'epoch') clearAgentTransport(sessionStorage);
      if (change === 'intent') h.session.journal.refusedControl(h.intent.key);
      return 'alice';
    });
    await expect(recoverV2Control(h.session, controller.signal)).rejects.toThrow();
    expect(h.control).not.toHaveBeenCalled();
    expect(h.observe).not.toHaveBeenCalled();
  },
);

it.each(['account', 'unavailable'] as const)(
  'does not publish acceptance after a post-response %s failure',
  async (failure) => {
    const h = await retained();
    h.currentUser.mockResolvedValueOnce('alice');
    if (failure === 'account') h.currentUser.mockResolvedValueOnce('bob');
    else h.currentUser.mockRejectedValueOnce(new Error('identity unavailable'));
    await expect(recoverV2Control(h.session, new AbortController().signal)).rejects.toThrow();
    expect(h.control).toHaveBeenCalledOnce();
    if (failure === 'account')
      expect(() => h.session.journal.readControl()).toThrow(IdentityChangedError);
    else expect(h.session.journal.readControl()).toEqual(h.intent);
  },
);

it('recovers an unavailable pre-dispatch check with a fresh read and no observation', async () => {
  const h = await retained(true);
  h.currentUser.mockRejectedValueOnce(new TransportError('temporary identity failure'));
  await recoverV2Control(h.session, new AbortController().signal);
  expect(h.currentUser).toHaveBeenCalledTimes(3);
  expect(h.control).toHaveBeenCalledOnce();
  expect(h.observe).not.toHaveBeenCalled();
});

it('retains a lost receipt and retries the exact key and body under fresh authority', async () => {
  const h = await retained();
  h.control.mockRejectedValueOnce(new TypeError('lost receipt'));
  await expect(recoverV2Control(h.session, new AbortController().signal)).rejects.toThrow();
  expect(h.session.journal.readControl()).toEqual(h.intent);
  await recoverV2Control(h.session, new AbortController().signal);
  expect(h.currentUser).toHaveBeenCalledTimes(3);
  expect(h.control).toHaveBeenCalledTimes(2);
  const [first, next] = h.control.mock.calls;
  expect(next![0]).toBe(first![0]);
  expect(next![1]!.body).toBe(first![1]!.body);
  expect(new Headers(next![1]!.headers).get('idempotency-key')).toBe(h.intent.key);
  expect(h.observe).not.toHaveBeenCalled();
});

it.each([
  [409, 'response_control_not_admitted'],
  [422, 'invalid_control_request'],
] as const)('clears only a definitive recovery refusal %s/%s', async (status, code) => {
  const h = await retained();
  h.control.mockResolvedValueOnce(
    json(
      {
        error: {
          type: 'invalid_request_error',
          code,
          message: 'Control refused',
          param: null,
        },
      },
      status,
    ),
  );
  await expect(recoverV2Control(h.session, new AbortController().signal)).rejects.toMatchObject({
    status,
    code,
  });
  expect(h.session.journal.readControl()).toBeNull();
  expect(h.currentUser).toHaveBeenCalledOnce();
  expect(h.control).toHaveBeenCalledOnce();
  expect(h.observe).not.toHaveBeenCalled();
});

it.each(['accepted', 'absent'] as const)(
  'does not issue requests when the retained control is %s',
  async (state) => {
    const h = await retained();
    if (state === 'accepted') h.session.journal.acceptedControl(h.intent.key, 'known-command');
    else h.session.journal.refusedControl(h.intent.key);
    await recoverV2Control(h.session, new AbortController().signal);
    expect(h.currentUser).not.toHaveBeenCalled();
    expect(h.control).not.toHaveBeenCalled();
  },
);
