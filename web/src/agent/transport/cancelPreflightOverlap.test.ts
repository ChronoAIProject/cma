import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { TransportError } from '@chronoai/cma-sdk';
import { harness, json } from './controller.fixture';
import { observation } from './controls.fixture';
import { submitV2Control } from './controls';
import { agentTransportJournal, clearAgentTransport } from './journal';
import { IdentityChangedError } from './identity';

beforeEach(() => {
  sessionStorage.clear();
  vi.useFakeTimers();
});
afterEach(() => vi.useRealTimers());
const target = { responseId: 'resp_1', commandId: 'cmd_1', turnSequence: 1 };

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

async function opened(retry = false) {
  const h = harness(sessionStorage, { enabled: retry, delay: () => 0 });
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  h.currentUser.mockClear();
  const submit = (signal = new AbortController().signal) =>
    submitV2Control(session, target, { type: 'cancel' }, 'original', signal);
  return { ...h, session, submit };
}

it.each(['identity', 'observation'] as const)(
  'waits for both fresh reads when %s finishes first',
  async (first) => {
    const h = await opened();
    const identity = deferred<string>();
    const observed = deferred<Response>();
    h.currentUser.mockReturnValueOnce(identity.promise);
    h.observe.mockReturnValueOnce(observed.promise);
    const result = h.submit();
    await vi.advanceTimersByTimeAsync(0);
    expect(h.currentUser).toHaveBeenCalledOnce();
    expect(h.observe).toHaveBeenCalledOnce();
    if (first === 'identity') identity.resolve('alice');
    else observed.resolve(json(observation()));
    await vi.advanceTimersByTimeAsync(0);
    expect(h.control).not.toHaveBeenCalled();
    expect(h.session.journal.readControl()).toBeNull();
    identity.resolve('alice');
    observed.resolve(json(observation()));
    await result;
    expect(h.currentUser).toHaveBeenCalledTimes(3);
    expect(h.control).toHaveBeenCalledOnce();
  },
);

it.each(['identity', 'observation'] as const)(
  'cancels the sibling and retains no intent when %s fails',
  async (failed) => {
    const h = await opened();
    const identity = deferred<string>();
    const observed = deferred<Response>();
    h.currentUser.mockReturnValueOnce(identity.promise);
    h.observe.mockReturnValueOnce(observed.promise);
    const result = h.submit().catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(0);
    const identitySignal = h.currentUser.mock.lastCall![0]!;
    const readSignal = h.fetch.mock.calls.find(([url]) => String(url).endsWith('/controls'))![1]!
      .signal!;
    if (failed === 'identity') identity.reject(new Error('identity unavailable'));
    else observed.reject(new TypeError('observation unavailable'));
    expect(await result).toBeInstanceOf(Error);
    // The failed request has already detached; the outstanding sibling aborts.
    expect(failed === 'identity' ? readSignal.aborted : identitySignal.aborted).toBe(true);
    // Late rejection of an uncooperative transport is still handled.
    identity.reject(new Error('late identity'));
    observed.reject(new Error('late observation'));
    await vi.advanceTimersByTimeAsync(0);
    expect(h.control).not.toHaveBeenCalled();
    expect(h.session.journal.readControl()).toBeNull();
  },
);

it.each(['account', 'epoch'] as const)(
  'rejects a late observation after the local %s changes',
  async (change) => {
    const h = await opened();
    const observed = deferred<Response>();
    h.observe.mockReturnValueOnce(observed.promise);
    const result = h.submit().catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(0);
    clearAgentTransport(sessionStorage);
    const replacement = agentTransportJournal(
      sessionStorage,
      change === 'account' ? 'bob' : 'alice',
      'agt_1',
    );
    replacement.select(true);
    const input = replacement.retain('new draft');
    observed.resolve(json(observation()));
    expect(await result).toBeInstanceOf(IdentityChangedError);
    expect(h.control).not.toHaveBeenCalled();
    expect(replacement.read()).toEqual(input);
    expect(replacement.readControl()).toBeNull();
  },
);

it.each(['identity', 'observation'] as const)(
  'does not consume either read after navigation aborts while %s is pending',
  async (pending) => {
    const h = await opened();
    const identity = deferred<string>();
    const observed = deferred<Response>();
    if (pending === 'identity') h.currentUser.mockReturnValueOnce(identity.promise);
    else h.observe.mockReturnValueOnce(observed.promise);
    const controller = new AbortController();
    const result = h.submit(controller.signal).catch((error: unknown) => error);
    await vi.advanceTimersByTimeAsync(0);
    controller.abort();
    identity.resolve('alice');
    observed.resolve(json(observation()));
    expect(await result).toMatchObject({ name: 'AbortError' });
    expect(h.control).not.toHaveBeenCalled();
    expect(h.session.journal.readControl()).toBeNull();
  },
);

it.each(['identity', 'observation'] as const)(
  'retries both fresh reads after a transient %s failure',
  async (failed) => {
    const h = await opened(true);
    const observe = vi.spyOn(h.session.responses, 'observeControls');
    if (failed === 'identity')
      h.currentUser.mockRejectedValueOnce(new TransportError('identity unavailable'));
    else h.observe.mockRejectedValueOnce(new TypeError('observation unavailable'));
    const result = h.submit();
    await vi.runAllTimersAsync();
    await result;
    // Early identity failure may abort SDK validation before the first fetch.
    expect(observe).toHaveBeenCalledTimes(2);
    expect(h.currentUser).toHaveBeenCalledTimes(4);
    expect(h.control).toHaveBeenCalledOnce();
    expect(h.session.journal.readControl()).toMatchObject({
      key: 'original',
      commandId: 'cmd_control',
    });
  },
);

it('dispatches after 200 ms with independent 100 ms identity and observation delays', async () => {
  const h = await opened();
  const delay = <T>(value: T) => new Promise<T>((resolve) => setTimeout(() => resolve(value), 100));
  h.currentUser.mockImplementation(() => delay('alice'));
  h.observe.mockImplementation(() => delay(json(observation())));
  const start = performance.now();
  let dispatchMs = -1;
  const submit = h.control.getMockImplementation()!;
  h.control.mockImplementation((...args) => {
    dispatchMs = performance.now() - start;
    return submit(...args);
  });
  const result = h.submit();
  await vi.advanceTimersByTimeAsync(199);
  expect(h.control).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect(h.control).toHaveBeenCalledOnce();
  expect(dispatchMs).toBe(200);
  await vi.runAllTimersAsync();
  await result;
});
