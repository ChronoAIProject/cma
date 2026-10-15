import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { fireEvent } from '@testing-library/react';
import { connectionClient, type ConnectionReport } from './client';
import {
  createPreparedConnectionHandoff,
  prepareAgentConnections,
  readAgentConnections,
} from './preparation';

const ready: ConnectionReport = {
  owner: 'alice',
  ready: true,
  checked_at: '2026-10-15T00:00:00Z',
  services: [],
};
beforeEach(() => vi.spyOn(connectionClient, 'check').mockResolvedValue(ready));
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('never offers an ordinary observation as a creation send handoff', () => {
  const take = createPreparedConnectionHandoff({
    report: ready,
    observedAt: performance.now(),
    prepared: false,
  });
  expect(take()).toBeUndefined();
});

it('consumes a failed freshness attempt permanently', () => {
  const now = vi.spyOn(performance, 'now').mockReturnValue(2001);
  const take = createPreparedConnectionHandoff({ report: ready, observedAt: 1000, prepared: true });
  expect(take()).toBeUndefined();
  now.mockReturnValue(1001);
  expect(take()).toBeUndefined();
});

it.each(['blur', 'visibilitychange'] as const)(
  'does not offer send reuse when %s happens before the personal view mounts',
  async (event) => {
    const context = {};
    const preparation = prepareAgentConnections(context, 'agt_1', 'alice');
    await Promise.resolve();
    fireEvent(event === 'blur' ? window : document, new Event(event));
    preparation.handoff();
    expect(
      await readAgentConnections(context, 'agt_1', 'alice', new AbortController().signal),
    ).toMatchObject({ report: ready, prepared: false });
    expect(connectionClient.check).toHaveBeenCalledOnce();
  },
);

it('stops watching an abandoned transferred result after its freshness window', async () => {
  vi.useFakeTimers();
  const removeWindow = vi.spyOn(window, 'removeEventListener');
  const removeDocument = vi.spyOn(document, 'removeEventListener');
  const context = {};
  prepareAgentConnections(context, 'agt_1', 'alice').handoff();
  await vi.advanceTimersByTimeAsync(1001);
  expect(removeWindow).toHaveBeenCalledWith('blur', expect.any(Function));
  expect(removeDocument).toHaveBeenCalledWith('visibilitychange', expect.any(Function));
  expect(
    await readAgentConnections(context, 'agt_1', 'alice', new AbortController().signal),
  ).toMatchObject({ report: ready, prepared: false });
});

it('cancels untransferred preparation and never reuses it for a later ordinary load', async () => {
  const context = {};
  const preparation = prepareAgentConnections(context, 'agt_1', 'alice');
  const signal = vi.mocked(connectionClient.check).mock.calls[0]![2];
  preparation.release();
  preparation.handoff();
  expect(signal?.aborted).toBe(true);
  expect(
    await readAgentConnections(context, 'agt_1', 'alice', new AbortController().signal),
  ).toMatchObject({ prepared: false });
  expect(connectionClient.check).toHaveBeenCalledTimes(2);
});

it.each(['owner', 'agent'] as const)('does not transfer across a changed %s', async (change) => {
  const context = {};
  prepareAgentConnections(context, 'agt_1', 'alice').handoff();
  const signal = vi.mocked(connectionClient.check).mock.calls[0]![2];
  expect(
    await readAgentConnections(
      context,
      change === 'agent' ? 'agt_2' : 'agt_1',
      change === 'owner' ? 'bob' : 'alice',
      new AbortController().signal,
    ),
  ).toMatchObject({ prepared: false });
  expect(signal?.aborted).toBe(true);
  expect(connectionClient.check).toHaveBeenCalledTimes(2);
});

it('cancels a transferred pending request when its authorized view is abandoned', async () => {
  let finish!: (report: ConnectionReport) => void;
  vi.mocked(connectionClient.check).mockReturnValueOnce(
    new Promise((resolve) => (finish = resolve)),
  );
  const context = {};
  prepareAgentConnections(context, 'agt_1', 'alice').handoff();
  const request = new AbortController();
  const result = readAgentConnections(context, 'agt_1', 'alice', request.signal);
  const refused = expect(result).rejects.toMatchObject({ name: 'AbortError' });
  request.abort();
  expect(vi.mocked(connectionClient.check).mock.calls[0]![2]?.aborted).toBe(true);
  finish(ready);
  await refused;
});

it('supersedes preparation without allowing old cleanup to cancel the new target', async () => {
  const context = {};
  const old = prepareAgentConnections(context, 'old', 'alice');
  const next = prepareAgentConnections(context, 'next', 'alice');
  old.release();
  old.handoff();
  next.handoff();
  expect(vi.mocked(connectionClient.check).mock.calls[0]![2]?.aborted).toBe(true);
  expect(vi.mocked(connectionClient.check).mock.calls[1]![2]?.aborted).toBe(false);
  expect(
    await readAgentConnections(context, 'next', 'alice', new AbortController().signal),
  ).toMatchObject({ prepared: true });
  expect(
    await readAgentConnections(context, 'next', 'alice', new AbortController().signal),
  ).toMatchObject({ prepared: false });
  expect(connectionClient.check).toHaveBeenCalledTimes(3);
});
