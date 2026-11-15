import { beforeEach, expect, it, vi } from 'vitest';
import { harness } from './controller.fixture';
import { IdentityChangedError } from './identity';
import { agentTransportJournal, clearAgentTransport } from './journal';

beforeEach(() => sessionStorage.clear());

function deferred() {
  let resolve!: (value: string) => void;
  const promise = new Promise<string>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

it('shares one bounded Cancel gate across 200 sessions while background work also advances', async () => {
  const h = harness();
  const sessions = await Promise.all(
    Array.from({ length: 200 }, (_, index) =>
      h.client.agentTransport!.open(`agt_${String(index)}`),
    ),
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
  const first = Promise.all(sessions.map((session) => session!.authorizeCancel()));
  await vi.waitFor(() => expect(reads).toHaveLength(2));
  const later = Promise.all(
    sessions.map((session) => session!.forSnapshotRefresh().authorizeCancel()),
  );
  await Promise.resolve();
  expect(reads).toHaveLength(2);
  reads[1]!.resolve('alice');
  await first;
  await vi.waitFor(() => expect(reads).toHaveLength(3));
  expect(active).toBe(2);
  reads[0]!.resolve('alice');
  await background;
  reads[2]!.resolve('alice');
  await later;
  expect(maximum).toBe(2);
  expect(active).toBe(0);
});

it('an abort-resistant background request cannot delay fresh Cancel checks', async () => {
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
  await session.authorizeCancel();
  expect(h.currentUser.mock.lastCall?.[0]?.aborted).toBe(false);
  blocked.resolve('alice');
});

it('cancelling a Cancel subscriber does not abort background authority or other Cancel subscribers', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const normal = deferred();
  const cancel = deferred();
  h.currentUser.mockReturnValueOnce(normal.promise).mockReturnValueOnce(cancel.promise);
  const background = session.authorize();
  await Promise.resolve();
  const controller = new AbortController();
  const aborted = expect(session.authorizeCancel(controller.signal)).rejects.toMatchObject({
    name: 'AbortError',
  });
  const retained = session.authorizeCancel();
  await Promise.resolve();
  const [normalCall, cancelCall] = h.currentUser.mock.calls.slice(-2);
  controller.abort();
  await aborted;
  expect(normalCall![0]!.aborted).toBe(false);
  expect(cancelCall![0]!.aborted).toBe(false);
  cancel.resolve('alice');
  await retained;
  normal.resolve('alice');
  await background;
});

it.each(['background', 'cancel'] as const)(
  'a changed account on %s invalidates an older response from the other gate',
  async (changed) => {
    const h = harness();
    const session = (await h.client.agentTransport!.open('agt_1'))!;
    const old = deferred();
    h.currentUser.mockReturnValueOnce(old.promise).mockResolvedValueOnce('bob');
    const pending = expect(
      changed === 'background' ? session.authorizeCancel() : session.authorize(),
    ).rejects.toBeInstanceOf(IdentityChangedError);
    await Promise.resolve();
    await expect(
      changed === 'background' ? session.authorize() : session.authorizeCancel(),
    ).rejects.toBeInstanceOf(IdentityChangedError);
    const replacement = agentTransportJournal(sessionStorage, 'bob', 'agt_1');
    replacement.select(true);
    const retained = replacement.retain('New account draft');
    old.resolve('alice');
    await pending;
    expect(replacement.read()).toEqual(retained);
    expect(() => session.journal.read()).toThrow(IdentityChangedError);
  },
);

it('same-account sign-out and sign-in invalidate both outstanding gates', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const normal = deferred();
  const cancel = deferred();
  h.currentUser.mockReturnValueOnce(normal.promise).mockReturnValueOnce(cancel.promise);
  const background = expect(session.authorize()).rejects.toBeInstanceOf(IdentityChangedError);
  const control = expect(session.authorizeCancel()).rejects.toBeInstanceOf(IdentityChangedError);
  await Promise.resolve();
  clearAgentTransport(sessionStorage);
  const replacement = agentTransportJournal(sessionStorage, 'alice', 'agt_1');
  replacement.select(true);
  const retained = replacement.retain('New sign-in draft');
  normal.resolve('alice');
  cancel.resolve('alice');
  await Promise.all([background, control]);
  expect(replacement.read()).toEqual(retained);
});
