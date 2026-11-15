import { fireEvent } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { connectionClient, ConnectionFailure, type ConnectionReport } from './client';
import {
  prepareAgentConnections,
  prepareAgentOpeningConnections,
  readAgentConnections,
} from './preparation';

// Existing fixtures model a fresh cache miss; cached responses have dedicated tests.
beforeEach(() => {
  vi.spyOn(connectionClient, 'open').mockImplementation((target, signal) =>
    connectionClient.check(target, undefined, signal),
  );
});

const ready: ConnectionReport = {
  owner: 'alice',
  ready: true,
  checked_at: '2026-11-16T00:00:00Z',
  services: [],
};
const releases: (() => void)[] = [];
function prepare(context = {}, agent = 'agt_1', owner = 'alice') {
  const value = prepareAgentOpeningConnections(context, agent, owner);
  releases.push(value.release);
  return { context, ...value };
}
const read = (context: object, agent = 'agt_1', owner = 'alice') =>
  readAgentConnections(context, agent, owner, new AbortController().signal);
beforeEach(() => vi.spyOn(connectionClient, 'check').mockResolvedValue(ready));
afterEach(() => {
  releases.splice(0).forEach((release) => release());
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('transfers the pending exact request once without a prepared Send capability', async () => {
  let finish!: (report: ConnectionReport) => void;
  vi.mocked(connectionClient.check).mockReturnValueOnce(
    new Promise((done) => {
      finish = done;
    }),
  );
  const { context } = prepare();
  const result = read(context);
  expect(connectionClient.check).toHaveBeenCalledOnce();
  finish(ready);
  expect(await result).toMatchObject({ report: ready, prepared: false });
  expect(await read(context)).toMatchObject({ prepared: false });
  expect(connectionClient.check).toHaveBeenCalledTimes(2);
});

it.each(['agent', 'owner', 'client'] as const)(
  'does not transfer to a different %s',
  async (change) => {
    const { context } = prepare();
    await read(
      change === 'client' ? {} : context,
      change === 'agent' ? 'agt_2' : 'agt_1',
      change === 'owner' ? 'bob' : 'alice',
    );
    expect(connectionClient.check).toHaveBeenCalledTimes(2);
    if (change !== 'client')
      expect(vi.mocked(connectionClient.check).mock.calls[0]![2]?.aborted).toBe(true);
  },
);

it.each([999, 2000, 2001])('requires a new check at invalid monotonic time %s', async (now) => {
  const clock = vi.spyOn(performance, 'now').mockReturnValue(1000);
  const { context } = prepare();
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
  clock.mockReturnValue(now);
  await read(context);
  expect(connectionClient.check).toHaveBeenCalledTimes(2);
});

it.each(['blur', 'visibilitychange'] as const)(
  'discards a completed result on %s',
  async (event) => {
    const { context } = prepare();
    await Promise.resolve();
    await Promise.resolve();
    fireEvent(event === 'blur' ? window : document, new Event(event));
    await read(context);
    expect(connectionClient.check).toHaveBeenCalledTimes(2);
  },
);

it('releases a completed unconsumed result after its window', async () => {
  vi.useFakeTimers();
  const { context } = prepare();
  await vi.advanceTimersByTimeAsync(1001);
  expect(vi.mocked(connectionClient.check).mock.calls[0]![2]?.aborted).toBe(true);
  await read(context);
  expect(connectionClient.check).toHaveBeenCalledTimes(2);
});

it('does not transfer cached readiness that expires while access is loading', async () => {
  const clock = vi.spyOn(performance, 'now').mockReturnValue(1000);
  vi.mocked(connectionClient.open).mockResolvedValueOnce({
    ...ready,
    verification: 'cached',
    valid_for_ms: 50,
  });
  const { context } = prepare();
  await new Promise((resolve) => setTimeout(resolve, 0));
  clock.mockReturnValue(1050);
  const observation = await read(context);
  expect(observation.report).toEqual(ready);
  expect(connectionClient.check).toHaveBeenCalledOnce();
});

it.each(['route', 'consumer'] as const)(
  'rejects a late success after %s cancellation',
  async (reason) => {
    let finish!: (report: ConnectionReport) => void;
    vi.mocked(connectionClient.check).mockReturnValueOnce(
      new Promise((done) => {
        finish = done;
      }),
    );
    const { context, release } = prepare();
    const consumer = new AbortController();
    const result = readAgentConnections(context, 'agt_1', 'alice', consumer.signal);
    const refusal = expect(result).rejects.toMatchObject({ name: 'AbortError' });
    if (reason === 'route') release();
    else consumer.abort();
    expect(vi.mocked(connectionClient.check).mock.calls[0]![2]?.aborted).toBe(true);
    finish(ready);
    await refusal;
  },
);

it('keeps ordinary fresh-read behavior after the authorized consumer takes the request', async () => {
  let finish!: (report: ConnectionReport) => void;
  vi.mocked(connectionClient.check).mockReturnValueOnce(
    new Promise((resolve) => {
      finish = resolve;
    }),
  );
  const { context } = prepare();
  const result = read(context);
  fireEvent(window, new Event('blur'));
  expect(vi.mocked(connectionClient.check).mock.calls[0]![2]?.aborted).toBe(false);
  finish(ready);
  expect(await result).toMatchObject({ report: ready, prepared: false });
  expect(connectionClient.check).toHaveBeenCalledOnce();
});

it('keeps a throttling failure and its cooldown across a fresh opening', async () => {
  const failure = new ConnectionFailure(429, 'rate_limited', 5000);
  vi.mocked(connectionClient.check).mockRejectedValueOnce(failure);
  const { context } = prepare();
  await expect(read(context)).rejects.toBe(failure);
  prepare(context);
  await expect(read(context)).rejects.toBe(failure);
  expect(connectionClient.check).toHaveBeenCalledOnce();
});

it('never replaces an exact transferred creation preparation', async () => {
  const context = {};
  const creation = prepareAgentConnections(context, 'agt_1', 'alice');
  creation.handoff();
  const opening = prepare(context);
  opening.release();
  expect(await read(context)).toMatchObject({ report: ready, prepared: true });
  expect(connectionClient.check).toHaveBeenCalledOnce();
});

it('keeps newer preparation intact when an old route releases late', async () => {
  const context = {};
  const old = prepare(context, 'old');
  prepare(context);
  old.release();
  expect(await read(context)).toMatchObject({ report: ready, prepared: false });
  expect(connectionClient.check).toHaveBeenCalledTimes(2);
  expect(vi.mocked(connectionClient.check).mock.calls[1]![2]?.aborted).toBe(false);
});

it('replaces an unrelated creation handoff without duplicating the new route check', async () => {
  const context = {};
  const creation = prepareAgentConnections(context, 'old', 'alice');
  creation.handoff();
  prepare(context);
  creation.release();
  expect(await read(context)).toMatchObject({ report: ready, prepared: false });
  expect(connectionClient.check).toHaveBeenCalledTimes(2);
  expect(vi.mocked(connectionClient.check).mock.calls[0]![2]?.aborted).toBe(true);
});

it('does not start speculative checks in a hidden document', () => {
  vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
  prepare();
  expect(connectionClient.check).not.toHaveBeenCalled();
});
