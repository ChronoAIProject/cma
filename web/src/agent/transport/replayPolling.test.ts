import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { TransportError, type V2AgentCursor } from '@chronoai/cma-sdk';
import { harness, cursor } from './controller.fixture';
import { followV2Agent } from './replay';

beforeEach(() => {
  sessionStorage.clear();
  vi.useFakeTimers();
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
