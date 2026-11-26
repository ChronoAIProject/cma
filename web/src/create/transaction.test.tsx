import { act, renderHook, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { CommandOptions } from '../api/client';
import { useCreationCommand } from './transaction';

it.each([
  { status: 401, code: 'unauthorized' },
  { status: 403, code: 'forbidden' },
  { status: 422, code: 'invalid_request' },
  { status: 429, code: 'rate_limited' },
])('retains an unresolved creation after a later $status refusal', async (refusal) => {
  const operation = vi
    .fn<(options: CommandOptions) => Promise<string>>()
    .mockRejectedValueOnce(new TypeError('response lost'))
    .mockRejectedValueOnce(refusal)
    .mockResolvedValueOnce('original workspace');
  const replacement = vi.fn().mockResolvedValue('different workspace');
  const onCreated = vi.fn();
  const { result } = renderHook(() => useCreationCommand(onCreated));

  await act(() => result.current.submit(operation));
  const originalKey = operation.mock.calls[0]![0].idempotencyKey;
  expect(originalKey).toEqual(expect.any(String));
  expect(result.current.locked).toBe(true);
  await act(async () => result.current.retry());
  await waitFor(() => expect(result.current.pending).toBe(false));
  expect(operation).toHaveBeenCalledTimes(2);
  expect(result.current.failure?.uncertain).toBe(true);
  expect(result.current.locked).toBe(true);
  expect(onCreated).not.toHaveBeenCalled();

  await act(() => result.current.submit(replacement));
  expect(replacement).not.toHaveBeenCalled();
  expect(operation.mock.calls.map(([options]) => options.idempotencyKey)).toEqual([
    originalKey,
    originalKey,
    originalKey,
  ]);
  expect(onCreated).toHaveBeenCalledExactlyOnceWith('original workspace');
  expect(result.current.locked).toBe(false);
  expect(result.current.failure).toBeUndefined();
});

it('keeps one request through recovery, transient failure and refusal until success', async () => {
  const operation = vi
    .fn<(options: CommandOptions) => Promise<string>>()
    .mockRejectedValueOnce({ status: 409, code: 'idempotency_recovery_required' })
    .mockRejectedValueOnce({ status: 503, code: 'unavailable' })
    .mockRejectedValueOnce({ status: 409, code: 'idempotency_in_flight' })
    .mockRejectedValueOnce({ status: 403, code: 'forbidden' })
    .mockResolvedValueOnce('recovered workspace');
  const onCreated = vi.fn();
  const { result } = renderHook(() => useCreationCommand(onCreated));
  await act(() => result.current.submit(operation));
  for (let attempt = 2; attempt <= 4; attempt++) {
    await act(async () => result.current.retry());
    await waitFor(() => expect(result.current.pending).toBe(false));
    expect(operation).toHaveBeenCalledTimes(attempt);
    expect(result.current.locked).toBe(true);
  }
  await act(async () => result.current.retry());
  await waitFor(() => expect(onCreated).toHaveBeenCalledExactlyOnceWith('recovered workspace'));
  expect(new Set(operation.mock.calls.map(([options]) => options.idempotencyKey)).size).toBe(1);

  const rejected = vi
    .fn<(options: CommandOptions) => Promise<string>>()
    .mockRejectedValue({ status: 422, code: 'invalid_request' });
  await act(() => result.current.submit(rejected));
  expect(result.current.locked).toBe(false);
  expect(result.current.failure?.uncertain).toBe(false);
  expect(rejected.mock.calls[0]![0].idempotencyKey).not.toBe(
    operation.mock.calls[0]![0].idempotencyKey,
  );
});

it('allows corrected input and a new key after an initial definite refusal', async () => {
  const rejected = vi
    .fn<(options: CommandOptions) => Promise<string>>()
    .mockRejectedValue({ status: 422, code: 'invalid_request' });
  const corrected = vi
    .fn<(options: CommandOptions) => Promise<string>>()
    .mockResolvedValue('corrected workspace');
  const onCreated = vi.fn();
  const { result } = renderHook(() => useCreationCommand(onCreated));
  await act(() => result.current.submit(rejected));
  expect(result.current.locked).toBe(false);
  expect(result.current.failure?.uncertain).toBe(false);
  await act(() => result.current.submit(corrected));
  expect(corrected.mock.calls[0]![0].idempotencyKey).not.toBe(
    rejected.mock.calls[0]![0].idempotencyKey,
  );
  expect(onCreated).toHaveBeenCalledExactlyOnceWith('corrected workspace');
});

it('admits only one concurrent recovery attempt for the original request', async () => {
  let finish!: (value: string) => void;
  const recovery = new Promise<string>((resolve) => {
    finish = resolve;
  });
  const operation = vi
    .fn<(options: CommandOptions) => Promise<string>>()
    .mockRejectedValueOnce(new TypeError('response lost'))
    .mockReturnValueOnce(recovery);
  const onCreated = vi.fn();
  const { result } = renderHook(() => useCreationCommand(onCreated));
  await act(() => result.current.submit(operation));
  let inFlight: Promise<void> | undefined;
  act(() => {
    inFlight = result.current.submit(operation);
    void result.current.submit(operation);
    result.current.retry();
  });
  expect(operation).toHaveBeenCalledTimes(2);
  expect(result.current.pending).toBe(true);
  expect(result.current.locked).toBe(true);
  await act(async () => {
    finish('recovered workspace');
    await inFlight;
  });
  expect(operation.mock.calls[1]![0].idempotencyKey).toBe(
    operation.mock.calls[0]![0].idempotencyKey,
  );
  expect(onCreated).toHaveBeenCalledExactlyOnceWith('recovered workspace');
  expect(result.current.locked).toBe(false);
});
