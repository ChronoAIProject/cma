import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { useAgentController } from './controller';

afterEach(() => vi.useRealTimers());

it('reattaches an accepted AG-UI turn after access recovery without sending it again', async () => {
  let runSignal: AbortSignal | undefined;
  const client = agentClient({
    runAgent: vi.fn(
      async (_request, options) =>
        new Promise<void>((resolve) => {
          runSignal = options.signal;
          options.onEvent({ type: 'RUN_STARTED', threadId: 'agt_1', runId: '5', cursor: 5 });
          options.signal!.addEventListener('abort', () => resolve(), { once: true });
        }),
    ),
  });
  const replay = vi.spyOn(client, 'replayAgent');
  const { result, rerender, unmount } = renderHook(
    ({ suspended }) => useAgentController(client, 'agt_1', { suspended }),
    { initialProps: { suspended: false } },
  );
  await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
  const accepted = vi.fn();
  let sent!: Promise<void>;
  act(() => {
    sent = result.current.send('Continue this accepted turn', accepted);
  });
  await waitFor(() => expect(accepted).toHaveBeenCalledOnce());
  const retained = result.current.state;
  rerender({ suspended: true });
  await act(() => sent);
  expect(runSignal?.aborted).toBe(true);
  expect(result.current.state).toBe(retained);
  expect(result.current.inputAvailability.allowed).toBe(false);

  rerender({ suspended: false });
  await waitFor(() =>
    expect(replay).toHaveBeenLastCalledWith('agt_1', expect.objectContaining({ after: 5 })),
  );
  expect(result.current.state).toMatchObject({
    status: 'ready',
    conversation: { activeRunId: '5', cursor: 5 },
  });
  expect(client.runAgent).toHaveBeenCalledOnce();
  expect(accepted).toHaveBeenCalledOnce();
  unmount();
});

it('retains the rendered snapshot while suspending runtime reads and both current and captured actions', async () => {
  vi.useFakeTimers();
  const streams: AbortSignal[] = [];
  const client = agentClient({
    runAgent: vi.fn(),
    interruptAgent: vi.fn(),
    stopAgent: vi.fn(),
    deleteAgent: vi.fn(),
    replayAgent: vi.fn(
      async (_id, options) =>
        new Promise<void>((resolve) => {
          streams.push(options.signal!);
          options.signal!.addEventListener('abort', () => resolve(), { once: true });
        }),
    ),
  });
  const readAgent = vi.spyOn(client, 'readAgent');
  const readSandbox = vi.spyOn(client, 'readSandbox');
  const { result, rerender, unmount } = renderHook(
    ({ suspended }) => useAgentController(client, 'agt_1', { suspended }),
    { initialProps: { suspended: false } },
  );
  await act(() => vi.advanceTimersByTimeAsync(0));
  const captured = result.current;
  expect(captured.state.status).toBe('ready');
  rerender({ suspended: true });
  expect(result.current.state).toBe(captured.state);
  expect(result.current.inputAvailability.allowed).toBe(false);
  expect(result.current.resourceAvailability.state).toBe('unavailable');
  expect(result.current.controlsUnavailable).toBe(true);
  expect(streams.length).toBeGreaterThan(0);
  expect(streams.every((signal) => signal.aborted)).toBe(true);
  const reads = [readAgent.mock.calls.length, readSandbox.mock.calls.length, streams.length];
  for (const controller of [captured, result.current]) {
    await act(async () => {
      await controller.send('Must not be sent');
      await controller.approve('approval', 'allow');
      await controller.interrupt();
      await controller.stop();
      await controller.delete();
    });
  }
  await act(() => vi.advanceTimersByTimeAsync(60_000));
  expect([readAgent.mock.calls.length, readSandbox.mock.calls.length, streams.length]).toEqual(
    reads,
  );
  for (const mutation of [
    client.runAgent,
    client.interruptAgent,
    client.stopAgent,
    client.deleteAgent,
  ])
    expect(mutation).not.toHaveBeenCalled();

  rerender({ suspended: false });
  await act(() => vi.advanceTimersByTimeAsync(0));
  expect(result.current.state).toBe(captured.state);
  expect(readAgent).toHaveBeenCalledTimes(reads[0]!);
  expect(result.current.inputAvailability.allowed).toBe(true);
  await act(() => captured.send('Old callback must still be refused'));
  expect(client.runAgent).not.toHaveBeenCalled();
  await act(() => result.current.send('Fresh explicit message'));
  expect(client.runAgent).toHaveBeenCalledOnce();
  unmount();
});
