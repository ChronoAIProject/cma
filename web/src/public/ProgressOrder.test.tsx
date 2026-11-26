import { act, renderHook, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { Launch } from '@cma/client';
import { PUBLIC_RECORD, publicClient } from '../test/public';
import { useRecordedProgress } from './useRecordedProgress';

const historical: Launch = { ...PUBLIC_RECORD, revision: 2, event_head: 3 };
const cancelled: Launch = {
  ...PUBLIC_RECORD,
  state: 'cancelled',
  revision: 3,
  event_head: 5,
};

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (cause: Error) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

it.each(['success', 'failure'] as const)(
  'retains newer progress while an old receipt refresh ends in %s',
  async (ending) => {
    const client = publicClient();
    const refresh = deferred<Launch>();
    vi.mocked(client.requests.read)
      .mockResolvedValueOnce(cancelled)
      .mockReturnValueOnce(refresh.promise);
    const { result, unmount } = renderHook(() => useRecordedProgress(client, cancelled.id));
    await waitFor(() => expect(result.current.record).toEqual(cancelled));
    const observedAt = result.current.observedAt;
    await act(async () => result.current.changed(historical));
    expect.soft(result.current.record).toEqual(cancelled);
    expect.soft(result.current.observedAt).toBe(observedAt);
    expect(client.requests.read).toHaveBeenCalledTimes(2);
    await act(async () => {
      if (ending === 'success') refresh.resolve(cancelled);
      else refresh.reject(new Error('Progress unavailable'));
    });
    expect(result.current.record).toEqual(cancelled);
    if (ending === 'success') expect(result.current.error).toBeUndefined();
    else {
      expect(result.current.error).toBeDefined();
      expect(result.current.observedAt).toBe(observedAt);
    }
    expect(client.requests.start).not.toHaveBeenCalled();
    expect(client.requests.cancel).not.toHaveBeenCalled();
    expect(client.requests.createOrResumeBound).not.toHaveBeenCalled();
    unmount();
  },
);

it.each([
  { ...historical, event_head: cancelled.event_head },
  { ...historical, revision: cancelled.revision + 1 },
])('keeps the accepted record when a refresh regresses a version counter: %j', async (older) => {
  const client = publicClient();
  vi.mocked(client.requests.read)
    .mockResolvedValueOnce(cancelled)
    .mockResolvedValueOnce(older)
    .mockResolvedValueOnce(cancelled);
  const { result, unmount } = renderHook(() => useRecordedProgress(client, cancelled.id));
  await waitFor(() => expect(result.current.record).toEqual(cancelled));
  const observedAt = result.current.observedAt;
  await act(async () => result.current.refresh());
  expect(result.current.record).toEqual(cancelled);
  expect(result.current.observedAt).toBe(observedAt);
  expect(result.current.error).toBeDefined();
  await act(async () => result.current.refresh());
  expect(result.current.record).toEqual(cancelled);
  expect(result.current.error).toBeUndefined();
  unmount();
});

it('accepts an identical replay and advancing versions without changing the receipt', async () => {
  const client = publicClient();
  vi.mocked(client.requests.read).mockResolvedValue(historical);
  const { result, unmount } = renderHook(() => useRecordedProgress(client, historical.id));
  await waitFor(() => expect(result.current.record).toEqual(historical));
  const receipt = structuredClone(historical);
  await act(async () => result.current.changed(receipt));
  expect(result.current.record).toEqual(historical);
  vi.mocked(client.requests.read).mockResolvedValue(cancelled);
  await act(async () => result.current.changed(cancelled));
  expect(result.current.record).toEqual(cancelled);
  expect(receipt).toEqual(historical);
  unmount();
});

it.each(['client', 'launch'] as const)(
  'resets the observation boundary on %s change',
  async (change) => {
    const client = publicClient();
    vi.mocked(client.requests.read).mockResolvedValue(cancelled);
    const { result, rerender, unmount } = renderHook(
      (props) => useRecordedProgress(props.client, props.id),
      { initialProps: { client, id: cancelled.id } },
    );
    await waitFor(() => expect(result.current.record).toEqual(cancelled));
    const late = result.current.changed;
    const nextClient = change === 'client' ? publicClient() : client;
    const next: Launch = {
      ...PUBLIC_RECORD,
      id: change === 'launch' ? 'lch_other_synthetic' : cancelled.id,
      state: 'draft',
    };
    const read = deferred<Launch>();
    vi.mocked(nextClient.requests.read).mockReturnValue(read.promise);
    rerender({ client: nextClient, id: next.id });
    expect(result.current.record).toBeUndefined();
    expect(result.current.observedAt).toBeUndefined();
    await act(async () => read.resolve(next));
    expect(result.current.record).toEqual(next);
    const reads = vi.mocked(nextClient.requests.read).mock.calls.length;
    await act(async () => late(historical));
    expect(result.current.record).toEqual(next);
    expect(nextClient.requests.read).toHaveBeenCalledTimes(reads);
    unmount();
  },
);

it('rejects a response for a different Launch without changing the observed identity', async () => {
  const client = publicClient();
  vi.mocked(client.requests.read)
    .mockResolvedValueOnce(cancelled)
    .mockResolvedValueOnce({ ...cancelled, id: 'lch_other_synthetic', revision: 4 });
  const { result, unmount } = renderHook(() => useRecordedProgress(client, cancelled.id));
  await waitFor(() => expect(result.current.record).toEqual(cancelled));
  await act(async () => result.current.refresh());
  expect(result.current.record).toEqual(cancelled);
  expect(result.current.error).toBeDefined();
  unmount();
});
