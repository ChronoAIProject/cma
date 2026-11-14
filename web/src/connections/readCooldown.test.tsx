import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { readWithConnectionCooldown } from './readCooldown';
import { ConnectionFailure, connectionClient, type ConnectionReport } from './client';
import { useAgentConnectionReadiness } from '../agent/connectionReadiness';

let now = 0;
const signal = () => new AbortController().signal;
const ready: ConnectionReport = {
  owner: 'alice',
  ready: true,
  checked_at: '2026-11-13T00:00:00Z',
  services: [],
};
beforeEach(() => {
  now = 0;
  vi.spyOn(performance, 'now').mockImplementation(() => now);
  vi.spyOn(Math, 'random').mockReturnValue(0);
});
afterEach(() => vi.restoreAllMocks());

it('does not cache successful readiness or authorization', async () => {
  const read = vi.fn().mockResolvedValue(ready);
  const context = {};
  await expect(readWithConnectionCooldown(context, 'alice', signal(), read)).resolves.toBe(ready);
  await expect(readWithConnectionCooldown(context, 'alice', signal(), read)).resolves.toBe(ready);
  expect(read).toHaveBeenCalledTimes(2);
});

it('retains a server deadline longer than the local cap across repeated retry attempts', async () => {
  const context = {};
  const error = new ConnectionFailure(429, 'rate_limited', 45000);
  const read = vi.fn().mockRejectedValueOnce(error).mockResolvedValue(ready);
  await expect(readWithConnectionCooldown(context, 'alice', signal(), read)).rejects.toBe(error);
  for (now of [1, 1000, 30000, 44999])
    await expect(readWithConnectionCooldown(context, 'alice', signal(), read)).rejects.toBe(error);
  expect(read).toHaveBeenCalledOnce();
  now = 45000;
  await expect(readWithConnectionCooldown(context, 'alice', signal(), read)).resolves.toBe(ready);
  expect(read).toHaveBeenCalledTimes(2);
});

it('does not share a failed owner or client scope with a new one', async () => {
  const context = {};
  const error = new ConnectionFailure(429, undefined, 45000);
  const read = vi.fn().mockRejectedValueOnce(error).mockResolvedValue(ready);
  await expect(readWithConnectionCooldown(context, 'alice', signal(), read)).rejects.toBe(error);
  await readWithConnectionCooldown({}, 'alice', signal(), read);
  await readWithConnectionCooldown(context, 'bob', signal(), read);
  expect(read).toHaveBeenCalledTimes(3);
});

it('preserves the longest overlapping deadline, even if an older request succeeds', async () => {
  const context = {};
  let finish!: (value: ConnectionReport) => void;
  const older = readWithConnectionCooldown(
    context,
    'alice',
    signal(),
    () =>
      new Promise<ConnectionReport>((resolve) => {
        finish = resolve;
      }),
  );
  const error = new ConnectionFailure(429, undefined, 45000);
  await expect(
    readWithConnectionCooldown(context, 'alice', signal(), async () => {
      throw error;
    }),
  ).rejects.toBe(error);
  finish(ready);
  await older;
  const read = vi.fn().mockResolvedValue(ready);
  now = 1000;
  await expect(readWithConnectionCooldown(context, 'alice', signal(), read)).rejects.toBe(error);
  expect(read).not.toHaveBeenCalled();
});

it('does not retain cancellation or permanent access refusal as a retryable observation', async () => {
  const context = {};
  const request = new AbortController();
  request.abort();
  const read = vi.fn().mockResolvedValue(ready);
  await expect(
    readWithConnectionCooldown(context, 'alice', request.signal, read),
  ).rejects.toMatchObject({ name: 'AbortError' });
  expect(read).not.toHaveBeenCalled();
  await expect(
    readWithConnectionCooldown(context, 'alice', signal(), async () => {
      throw new ApiError(403, 'Refused');
    }),
  ).rejects.toMatchObject({ status: 403 });
  await expect(readWithConnectionCooldown(context, 'alice', signal(), read)).resolves.toBe(ready);
});

it('keeps the deadline when the opening view remounts or manually retries', async () => {
  const context = {};
  const check = vi
    .spyOn(connectionClient, 'check')
    .mockRejectedValueOnce(new ConnectionFailure(429, 'rate_limited', 45000))
    .mockResolvedValue(ready);
  const mount = () =>
    renderHook(() => useAgentConnectionReadiness('agt_1', 'alice', context, true));
  const first = mount();
  await waitFor(() => expect(first.result.current.error).toBeTruthy());
  first.unmount();
  const next = mount();
  await waitFor(() => expect(next.result.current.error).toBeTruthy());
  act(() => next.result.current.retry());
  await waitFor(() => expect(next.result.current.error).toBeTruthy());
  expect(check).toHaveBeenCalledOnce();
  expect(next.result.current.verified).toBeUndefined();
  now = 45000;
  act(() => next.result.current.retry());
  await waitFor(() => expect(next.result.current.verified).toEqual(ready));
  expect(check).toHaveBeenCalledTimes(2);
});
