import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, ProtocolError, TimeoutError, TransportError } from '@cma/client';
import { AgentLifecycleCreateError } from '../api/client';
import { agentClient } from '../test/agent';
import { lifecycleStorageKey, useLifecycleIntent } from './lifecycleIntent';
import { lifecycleFailureMessage } from './lifecycleFailure';

const submitMessage =
  "We couldn't confirm your task was received. Use Recover original task to resend the same request.";
const statusMessage = "Task status couldn't be read. Use Retry task status to check again.";
const recoverAction = { label: 'Recover original task', kind: 'recover' } as const;
const statusAction = { label: 'Retry task status', kind: 'status' } as const;
const body = { name: 'New task', initial_message: 'Keep my exact task' };

afterEach(() => {
  sessionStorage.clear();
  vi.useRealTimers();
});

function observe(overrides: Parameters<typeof agentClient>[0]) {
  const client = agentClient(overrides);
  return renderHook(() => useLifecycleIntent(client, 'owner'));
}

const unknownFailures = [
  new TypeError('Failed to fetch'),
  new TypeError('Load failed'),
  new Error('Unexpected transport diagnostic'),
  new DOMException('The operation timed out', 'TimeoutError'),
  new TimeoutError(5000),
  new ApiError(503, ''),
  new ApiError(503, 'HTTP 503'),
  new ApiError(503, 'Agent lifecycle status failed (503).'),
  new ApiError(408, 'Request Timeout'),
  new ApiError(504, 'Gateway Timeout'),
  new ApiError(503, 'upstream_unavailable', 'upstream_unavailable'),
  new ApiError(503, 'upstream_unavailable'),
  new ProtocolError('HTTP 503 problem document carried no detail'),
  { unexpected: true },
];

it.each(unknownFailures)(
  'explains an uncertain submit and recovers the same request for %s',
  async (error) => {
    const create = vi.fn().mockRejectedValueOnce(error).mockResolvedValue({ id: 'operation' });
    const { result } = observe({ createAgentLifecycle: create });
    await act(() => result.current.submit(body));
    expect(lifecycleFailureMessage(result.current.failure, recoverAction)).toBe(submitMessage);
    const original = result.current.intent;
    expect(original?.body).toEqual(body);
    expect(original?.receipt).toBeUndefined();
    expect(create).toHaveBeenCalledTimes(1);
    await act(() => result.current.submit());
    expect(result.current.intent?.key).toBe(original?.key);
    expect(result.current.intent?.receipt?.id).toBe('operation');
    expect(create.mock.calls[1]?.[0]).toEqual(create.mock.calls[0]?.[0]);
    expect(create.mock.calls[1]?.[1].idempotencyKey).toBe(create.mock.calls[0]?.[1].idempotencyKey);
  },
);

it.each(unknownFailures)(
  'explains a status read failure and retries without resubmitting for %s',
  async (error) => {
    sessionStorage.setItem(
      lifecycleStorageKey('owner'),
      JSON.stringify({
        version: 1,
        intent: { key: 'original-key', body, receipt: { id: 'operation' } },
      }),
    );
    const create = vi.fn();
    const read = vi
      .fn()
      .mockRejectedValueOnce(error)
      .mockResolvedValue({ id: 'operation', status: 'failed', phase: 'sandbox_ready', events: [] });
    const { result } = observe({ createAgentLifecycle: create, readAgentLifecycle: read });
    await waitFor(() =>
      expect(lifecycleFailureMessage(result.current.failure, statusAction)).toBe(statusMessage),
    );
    expect(result.current.intent?.key).toBe('original-key');
    act(() => result.current.retryStatus());
    await waitFor(() => expect(result.current.failure).toBeUndefined());
    expect(read).toHaveBeenCalledTimes(2);
    expect(read.mock.calls.every(([id]) => id === 'operation')).toBe(true);
    expect(result.current.intent?.key).toBe('original-key');
    expect(create).not.toHaveBeenCalled();
  },
);

it.each([
  new ApiError(403, 'Your account cannot access this Sandbox.'),
  new ApiError(503, 'The selected image is being prepared. Please try again shortly.'),
  new AgentLifecycleCreateError('Choose another repository before starting.', 422),
  new ProtocolError('Invalid operation identity'),
  new ProtocolError('Agent setup identity could not be verified.'),
  new ProtocolError('Agent Manager returned an invalid lifecycle operation.'),
  new Error('Agent setup identity changed. Your original request has been saved.'),
])('preserves deliberate and server-provided sentences: $message', async (error) => {
  const { result } = observe({
    createAgentLifecycle: async () => {
      throw error;
    },
  });
  await act(() => result.current.submit(body));
  expect(result.current.failure).toEqual({ kind: 'message', message: error.message });
  if (error instanceof AgentLifecycleCreateError) expect(result.current.intent).toBeUndefined();
  else expect(result.current.intent?.body).toEqual(body);
});

it.each([400, 422])(
  'explains a detail-free %i refusal without offering recovery of a cleared intent',
  async (status) => {
    const { result } = observe({
      createAgentLifecycle: async () => {
        throw new AgentLifecycleCreateError('invalid_request', status, 'invalid_request');
      },
    });
    await act(() => result.current.submit(body));
    expect(result.current.failure).toEqual({
      kind: 'message',
      message: 'Your task could not be started. Review it and try again.',
    });
    expect(result.current.intent).toBeUndefined();
  },
);

it('keeps automatic retry timing and the same retained receipt after a network status failure', async () => {
  vi.useFakeTimers();
  sessionStorage.setItem(
    lifecycleStorageKey('owner'),
    JSON.stringify({
      version: 1,
      intent: { key: 'original-key', body, receipt: { id: 'operation' } },
    }),
  );
  const create = vi.fn();
  const read = vi
    .fn()
    .mockRejectedValueOnce(new TransportError('Failed to fetch'))
    .mockResolvedValue({ id: 'operation', status: 'failed', phase: 'sandbox_ready', events: [] });
  const { result } = observe({ createAgentLifecycle: create, readAgentLifecycle: read });
  await act(async () => vi.advanceTimersByTimeAsync(499));
  expect(lifecycleFailureMessage(result.current.failure, statusAction)).toBe(statusMessage);
  expect(read).toHaveBeenCalledTimes(1);
  await act(async () => vi.advanceTimersByTimeAsync(1));
  expect(read).toHaveBeenCalledTimes(2);
  expect(result.current.failure).toBeUndefined();
  expect(result.current.intent?.key).toBe('original-key');
  expect(create).not.toHaveBeenCalled();
});

const statuses = [401, 403, 404, 409, 413, 429, 500, 503, 504] as const;
const cases = (['ApiError', 'AgentLifecycleCreateError'] as const).flatMap((type) =>
  [true, false].flatMap((task) => statuses.map((status) => ({ type, task, status }))),
);

it.each(cases)(
  'distinguishes submit refusal from uncertainty for $type $status, task=$task',
  async ({ type, task, status }) => {
    const error =
      type === 'ApiError' ? new ApiError(status, '') : new AgentLifecycleCreateError('', status);
    const create = vi.fn().mockRejectedValue(error);
    const { result } = observe({ createAgentLifecycle: create });
    const request = task ? body : { name: 'New agent' };
    await act(() => result.current.submit(request));
    const original = result.current.intent;
    const message = lifecycleFailureMessage(
      result.current.failure,
      task ? recoverAction : { label: 'Recover creation', kind: 'recover' },
    );
    expect(message).toBe(
      status < 500 && status !== 429
        ? `Agent Manager refused this ${task ? 'task' : 'Agent creation'} (${status}). Your original request is saved.`
        : task
          ? submitMessage
          : "We couldn't confirm your Agent was created. Use Recover creation to resend the same request.",
    );
    expect(original?.body).toEqual(request);
    expect(JSON.parse(sessionStorage.getItem(lifecycleStorageKey('owner'))!).intent.key).toBe(
      original?.key,
    );
    await act(() => result.current.submit());
    expect(create).toHaveBeenCalledTimes(2);
    expect(create.mock.calls[1]?.[0]).toEqual(request);
    expect(create.mock.calls[1]?.[1].idempotencyKey).toBe(create.mock.calls[0]?.[1].idempotencyKey);
    expect(result.current.intent?.key).toBe(original?.key);
  },
);

it.each(cases)(
  'describes only the failed status read for $type $status, task=$task',
  async ({ type, task, status }) => {
    const error =
      type === 'ApiError' ? new ApiError(status, '') : new AgentLifecycleCreateError('', status);
    sessionStorage.setItem(
      lifecycleStorageKey('owner'),
      JSON.stringify({
        version: 1,
        intent: {
          key: 'original-key',
          body: task ? body : { name: 'New agent' },
          receipt: { id: 'operation' },
        },
      }),
    );
    const create = vi.fn();
    const read = vi
      .fn()
      .mockRejectedValueOnce(error)
      .mockResolvedValue({ id: 'operation', status: 'failed', phase: 'sandbox_ready', events: [] });
    const { result } = observe({ createAgentLifecycle: create, readAgentLifecycle: read });
    const action = task ? statusAction : { label: 'Retry status', kind: 'status' as const };
    await waitFor(() =>
      expect(lifecycleFailureMessage(result.current.failure, action)).toBe(
        task ? statusMessage : "Agent status couldn't be read. Use Retry status to check again.",
      ),
    );
    expect(result.current.intent?.key).toBe('original-key');
    act(() => result.current.retryStatus());
    await waitFor(() => expect(result.current.failure).toBeUndefined());
    expect(read).toHaveBeenCalledTimes(2);
    expect(read.mock.calls.every(([id]) => id === 'operation')).toBe(true);
    expect(create).not.toHaveBeenCalled();
    expect(result.current.intent?.key).toBe('original-key');
  },
);
