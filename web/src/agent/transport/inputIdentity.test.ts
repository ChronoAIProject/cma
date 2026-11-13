import { beforeEach, expect, it, vi } from 'vitest';
import { accepted, harness } from './controller.fixture';
import { IdentityChangedError, isIdentityChanged } from './identity';
import { agentTransportJournal, clearAgentTransport } from './journal';

beforeEach(() => sessionStorage.clear());

function deferred() {
  let resolve!: (value: string) => void;
  const promise = new Promise<string>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

it('bounds 200 Send sessions to one input lane while background and Cancel remain independent', async () => {
  const h = harness();
  const fetch = h.fetch.getMockImplementation()!;
  h.fetch.mockImplementation((input, init) =>
    /^\/api\/v2\/agents\/agt_\d+\/responses$/.test(String(input))
      ? h.post(init)
      : fetch(input, init),
  );
  h.post.mockImplementation(async () => accepted());
  const sessions = await Promise.all(
    Array.from({ length: 200 }, (_, i) => h.client.agentTransport!.open(`agt_${String(i)}`)),
  );
  const reads: ReturnType<typeof deferred>[] = [];
  let active = 0;
  let maximum = 0;
  h.currentUser.mockImplementation(() => {
    const read = deferred();
    reads.push(read);
    maximum = Math.max(maximum, ++active);
    return read.promise.finally(() => {
      active -= 1;
    });
  });
  const background = Promise.all(sessions.map((session) => session!.authorize()));
  await vi.waitFor(() => expect(reads).toHaveLength(1));
  const acceptedInput = vi.fn();
  const sends = Promise.all(sessions.map((session) => session!.send('input', acceptedInput)));
  await vi.waitFor(() => expect(reads).toHaveLength(2));
  const cancel = Promise.all(sessions.map((session) => session!.authorizeCancel()));
  await vi.waitFor(() => expect(reads).toHaveLength(3));
  reads[1]!.resolve('alice');
  await vi.waitFor(() => expect(reads).toHaveLength(4));
  expect(h.post).toHaveBeenCalledTimes(200);
  expect(acceptedInput).not.toHaveBeenCalled();
  reads[3]!.resolve('alice');
  await sends;
  expect(acceptedInput).toHaveBeenCalledTimes(200);
  const recovery = sessions[0]!.forSnapshotRefresh().recover(acceptedInput);
  await vi.waitFor(() => expect(reads).toHaveLength(5));
  reads[4]!.resolve('alice');
  await recovery;
  expect(h.post).toHaveBeenCalledTimes(200);
  expect(active).toBe(2);
  reads[0]!.resolve('alice');
  reads[2]!.resolve('alice');
  await Promise.all([background, cancel]);
  expect(maximum).toBe(3);
  expect(active).toBe(0);
});

it('an abort-resistant background read cannot hold Send or its receipt check', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const blocked = deferred();
  h.currentUser.mockReturnValueOnce(blocked.promise);
  const controller = new AbortController();
  const background = expect(session.authorize(controller.signal)).rejects.toMatchObject({
    name: 'AbortError',
  });
  await Promise.resolve();
  controller.abort();
  await background;
  const onAccepted = vi.fn();
  await session.send('input', onAccepted);
  expect(h.post).toHaveBeenCalledOnce();
  expect(onAccepted).toHaveBeenCalledOnce();
  blocked.resolve('alice');
});

it.each(['background', 'cancel'] as const)(
  'a changed account on %s invalidates a delayed Send authorization',
  async (lane) => {
    const h = harness();
    const session = (await h.client.agentTransport!.open('agt_1'))!;
    const delayed = deferred();
    h.currentUser.mockReturnValueOnce(delayed.promise).mockResolvedValueOnce('bob');
    const send = expect(session.send('old draft', vi.fn())).rejects.toBeInstanceOf(
      IdentityChangedError,
    );
    await Promise.resolve();
    await expect(
      lane === 'cancel' ? session.authorizeCancel() : session.authorize(),
    ).rejects.toBeInstanceOf(IdentityChangedError);
    const replacement = agentTransportJournal(sessionStorage, 'bob', 'agt_1');
    replacement.select(true);
    const retained = replacement.retain('new draft');
    delayed.resolve('alice');
    await send;
    expect(h.post).not.toHaveBeenCalled();
    expect(replacement.read()).toEqual(retained);
  },
);

it.each([1, 2])('same-account logout/login invalidates Send boundary %s', async (boundary) => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const delayed = deferred();
  let reached!: () => void;
  const ready = new Promise<void>((resolve) => {
    reached = resolve;
  });
  if (boundary === 2) h.currentUser.mockResolvedValueOnce('alice');
  h.currentUser.mockImplementationOnce(() => {
    reached();
    return delayed.promise;
  });
  const onAccepted = vi.fn();
  const send = session.send('old draft', onAccepted).catch((error: unknown) => error);
  await ready;
  clearAgentTransport(sessionStorage);
  const replacement = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
  replacement.select(true);
  const retained = replacement.retain('new sign-in draft');
  delayed.resolve('alice');
  expect(isIdentityChanged(await send)).toBe(true);
  expect(onAccepted).not.toHaveBeenCalled();
  expect(h.post).toHaveBeenCalledTimes(boundary === 1 ? 0 : 1);
  expect(replacement.read()).toEqual(retained);
});

it('Send account invalidation suppresses delayed background and Cancel responses', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const background = deferred();
  const cancel = deferred();
  h.currentUser
    .mockReturnValueOnce(background.promise)
    .mockReturnValueOnce(cancel.promise)
    .mockResolvedValueOnce('bob');
  const old = expect(session.authorize()).rejects.toBeInstanceOf(IdentityChangedError);
  await Promise.resolve();
  const stop = expect(session.authorizeCancel()).rejects.toBeInstanceOf(IdentityChangedError);
  await Promise.resolve();
  await expect(session.send('input', vi.fn())).rejects.toBeInstanceOf(IdentityChangedError);
  background.resolve('alice');
  cancel.resolve('alice');
  await Promise.all([old, stop]);
  expect(h.post).not.toHaveBeenCalled();
});
