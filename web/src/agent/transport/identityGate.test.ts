import { beforeEach, expect, it, vi } from 'vitest';
import { createIdentityGate, identityGateDiagnostics, type CurrentUser } from './identityGate';
import { harness } from './controller.fixture';
import { IdentityChangedError } from './identity';

function deferred() {
  let resolve!: (user: string | undefined) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<string | undefined>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

beforeEach(() => sessionStorage.clear());

it('batches synchronous callers into a request started after every call', async () => {
  const current = vi.fn<CurrentUser>().mockResolvedValue('alice');
  const gate = createIdentityGate(current);
  const before = identityGateDiagnostics();
  const checks = Array.from({ length: 10 }, () => gate.check());
  expect(current).not.toHaveBeenCalled();
  expect(await Promise.all(checks)).toEqual(Array(10).fill('alice'));
  expect(current).toHaveBeenCalledOnce();
  expect(identityGateDiagnostics().joined - before.joined).toBe(9);
  expect(identityGateDiagnostics().requests - before.requests).toBe(1);
});

it('queues callers arriving during a read and never returns an older proof to them', async () => {
  const first = deferred();
  const second = deferred();
  const current = vi
    .fn<CurrentUser>()
    .mockReturnValueOnce(first.promise)
    .mockReturnValue(second.promise);
  const gate = createIdentityGate(current);
  const earlier = gate.check();
  await Promise.resolve();
  const later = [gate.check(), gate.check()];
  expect(current).toHaveBeenCalledOnce();
  first.resolve('alice');
  expect(await earlier).toBe('alice');
  await Promise.resolve();
  expect(current).toHaveBeenCalledTimes(2);
  second.resolve('bob');
  expect(await Promise.all(later)).toEqual(['bob', 'bob']);
});

it('isolates cancellation and aborts the underlying request only after the last subscriber leaves', async () => {
  const pending = deferred();
  const current = vi.fn<CurrentUser>().mockReturnValue(pending.promise);
  const gate = createIdentityGate(current);
  const one = new AbortController();
  const two = new AbortController();
  const first = expect(gate.check(one.signal)).rejects.toMatchObject({ name: 'AbortError' });
  const second = gate.check(two.signal);
  await Promise.resolve();
  one.abort();
  await first;
  expect(current.mock.calls[0]![0]!.aborted).toBe(false);
  pending.resolve('alice');
  expect(await second).toBe('alice');
  expect(current.mock.calls[0]![0]!.aborted).toBe(false);

  const last = new AbortController();
  const read = expect(gate.check(last.signal)).rejects.toMatchObject({ name: 'AbortError' });
  await Promise.resolve();
  last.abort();
  await read;
  expect(current.mock.calls[1]![0]!.aborted).toBe(true);
});

it('drops cancelled queued batches without overlapping an abort-resistant in-flight read', async () => {
  const pending = deferred();
  const current = vi
    .fn<CurrentUser>()
    .mockReturnValueOnce(pending.promise)
    .mockResolvedValue('bob');
  const gate = createIdentityGate(current);
  const active = new AbortController();
  const first = expect(gate.check(active.signal)).rejects.toMatchObject({ name: 'AbortError' });
  await Promise.resolve();
  const queued = new AbortController();
  const cancelled = expect(gate.check(queued.signal)).rejects.toMatchObject({ name: 'AbortError' });
  queued.abort();
  active.abort();
  await Promise.all([first, cancelled]);
  const next = gate.check();
  await Promise.resolve();
  expect(current).toHaveBeenCalledOnce();
  pending.resolve('alice');
  expect(await next).toBe('bob');
  expect(current).toHaveBeenCalledTimes(2);
});

it('does not issue a request for already cancelled or synchronously cancelled callers', async () => {
  const current = vi.fn<CurrentUser>();
  const gate = createIdentityGate(current);
  const request = new AbortController();
  const pending = expect(gate.check(request.signal)).rejects.toMatchObject({ name: 'AbortError' });
  request.abort();
  await pending;
  await expect(gate.check(request.signal)).rejects.toMatchObject({ name: 'AbortError' });
  expect(current).not.toHaveBeenCalled();
});

it.each([
  new Error('401'),
  new Error('403'),
  new Error('503 identity_unavailable'),
  new DOMException('timeout', 'TimeoutError'),
])('propagates %s to its batch without caching failure', async (error) => {
  const current = vi.fn<CurrentUser>().mockRejectedValueOnce(error).mockResolvedValue('alice');
  const gate = createIdentityGate(current);
  await expect(Promise.all([gate.check(), gate.check()])).rejects.toBe(error);
  await expect(gate.check()).resolves.toBe('alice');
  expect(current).toHaveBeenCalledTimes(2);
});

it('recovers from a synchronous provider exception', async () => {
  const current = vi
    .fn<CurrentUser>()
    .mockImplementationOnce(() => {
      throw new Error('offline');
    })
    .mockResolvedValue('alice');
  const gate = createIdentityGate(current);
  await expect(gate.check()).rejects.toThrow('offline');
  await expect(gate.check()).resolves.toBe('alice');
});

it('shares checks across Agents and invalidates every waiting journal on an account change', async () => {
  const h = harness();
  const first = (await h.client.agentTransport!.open('agt_1'))!;
  const second = (await h.client.agentTransport!.open('agt_2'))!;
  first.journal.retain('retained input');
  h.currentUser.mockClear().mockResolvedValue('bob');
  const results = await Promise.allSettled([
    first.authorize(),
    second.authorize(),
    first.authorize(),
  ]);
  expect(h.currentUser).toHaveBeenCalledOnce();
  for (const result of results) {
    expect(result.status).toBe('rejected');
    if (result.status === 'rejected') expect(result.reason).toBeInstanceOf(IdentityChangedError);
  }
  expect(sessionStorage.length).toBe(0);
  expect(() => first.journal.read()).toThrow(IdentityChangedError);
  expect(() => second.journal.read()).toThrow(IdentityChangedError);
});
