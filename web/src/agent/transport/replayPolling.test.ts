import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { TransportError, type V2AgentCursor } from '@chronoai/cma-sdk';
import { harness, cursor } from './controller.fixture';
import { followV2Agent } from './replay';

beforeEach(() => {
  sessionStorage.clear();
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(1);
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it.each([false, true])(
  'backs off streams that close without progress (failure=%s)',
  async (fail) => {
    const h = harness();
    const session = (await h.client.agentTransport!.open('agt_1'))!;
    const events = vi.spyOn(session.events, 'events').mockImplementation(async () => {
      if (fail) throw new TransportError('Fixture disconnect');
    });
    const abort = new AbortController();
    const onFailure = vi.fn();
    const following = followV2Agent(session, cursor(0), abort.signal, vi.fn(), onFailure);
    await vi.advanceTimersByTimeAsync(30000);
    expect(events).toHaveBeenCalledTimes(7);
    expect(onFailure).toHaveBeenCalledWith(expect.stringContaining('reconnecting'));
    abort.abort();
    await following;
    await vi.advanceTimersByTimeAsync(30000);
    expect(events).toHaveBeenCalledTimes(7);
    expect(h.post).not.toHaveBeenCalled();
  },
);

it('resets reconnect delay only when the durable cursor advances', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const events = vi.spyOn(session.events, 'events').mockImplementation(async (_id, options) => {
    options.onCursor?.(cursor(0) as V2AgentCursor);
  });
  const abort = new AbortController();
  const following = followV2Agent(session, cursor(0), abort.signal, vi.fn(), vi.fn());
  await vi.advanceTimersByTimeAsync(3500);
  expect(events).toHaveBeenCalledTimes(4);
  events.mockImplementationOnce(async (_id, options) => {
    options.onCursor?.(cursor(1) as V2AgentCursor);
  });
  await vi.advanceTimersByTimeAsync(4000);
  const count = events.mock.calls.length;
  await vi.advanceTimersByTimeAsync(250);
  expect(events).toHaveBeenCalledTimes(count + 1);
  abort.abort();
  await following;
});

it('keeps an existing stream but suspends reconnects while hidden', async () => {
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const events = vi.spyOn(session.events, 'events').mockResolvedValue(undefined);
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
  const abort = new AbortController();
  const following = followV2Agent(session, cursor(0), abort.signal, vi.fn(), vi.fn());
  await vi.advanceTimersByTimeAsync(30000);
  expect(events).not.toHaveBeenCalled();
  hidden.mockReturnValue(false);
  document.dispatchEvent(new Event('visibilitychange'));
  await vi.advanceTimersByTimeAsync(0);
  expect(events).toHaveBeenCalledOnce();
  hidden.mockReturnValue(true);
  await vi.advanceTimersByTimeAsync(30000);
  expect(events).toHaveBeenCalledOnce();
  abort.abort();
  await following;
});

it.each([0, 0.999])('bounds every reconnect interval at jitter input %s', async (random) => {
  vi.mocked(Math.random).mockReturnValue(random);
  const h = harness();
  const session = (await h.client.agentTransport!.open('agt_1'))!;
  const started: number[] = [];
  vi.spyOn(session.events, 'events').mockImplementation(async () => {
    started.push(Date.now());
    throw new TransportError('Fixture disconnect');
  });
  const abort = new AbortController();
  const following = followV2Agent(session, cursor(0), abort.signal, vi.fn(), vi.fn());
  await vi.advanceTimersByTimeAsync(60000);
  expect(started.length).toBeGreaterThanOrEqual(7);
  for (let i = 1; i < started.length; i++) {
    const cap = Math.min(250 * 2 ** i, 10000);
    const interval = started[i]! - started[i - 1]!;
    expect(interval).toBeGreaterThanOrEqual(cap / 2);
    expect(interval).toBeLessThanOrEqual(cap);
  }
  expect(started[1]! - started[0]!).toBe(Math.floor(500 * (0.5 + random * 0.5)));
  abort.abort();
  await following;
  const count = started.length;
  await vi.advanceTimersByTimeAsync(30000);
  expect(started).toHaveLength(count);
  expect(h.post).not.toHaveBeenCalled();
});

it('separates simultaneous followers after the same disconnect', async () => {
  vi.mocked(Math.random).mockReturnValueOnce(0).mockReturnValueOnce(0.999).mockReturnValue(0.5);
  const followers = await Promise.all(
    [0, 1].map(async () => {
      const h = harness();
      const session = (await h.client.agentTransport!.open('agt_1'))!;
      const started: number[] = [];
      const events = vi.spyOn(session.events, 'events').mockImplementation(async () => {
        started.push(Date.now());
      });
      const abort = new AbortController();
      return { h, session, started, events, abort };
    }),
  );
  const running = followers.map(({ session, abort }) =>
    followV2Agent(session, cursor(0), abort.signal, vi.fn(), vi.fn()),
  );
  await vi.advanceTimersByTimeAsync(500);
  expect(followers[0]!.started[0]).toBe(followers[1]!.started[0]);
  expect(followers.map(({ started }) => started.length)).toEqual([2, 2]);
  expect(followers[0]!.started[1]).not.toBe(followers[1]!.started[1]);
  for (const { abort } of followers) abort.abort();
  await Promise.all(running);
  await vi.advanceTimersByTimeAsync(30000);
  for (const { events, h } of followers) {
    expect(events).toHaveBeenCalledTimes(2);
    expect(h.post).not.toHaveBeenCalled();
  }
});
