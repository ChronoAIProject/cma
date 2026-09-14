import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import type { AgentLiveEvent, SandboxDetail } from '../api/types';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, agentClient } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { useAgentController } from './controller';

afterEach(() => vi.useRealTimers());

const saving: SandboxDetail = {
  ...sandboxFixture(),
  lifecycle: {
    pause: { allowed: false, reason: 'checkpoint_in_progress' },
    revive: { allowed: false, reason: 'not_paused' },
    checkpoint: { phase: 'capture', startedAt: '2026-09-08T00:00:00Z', epoch: 8 },
  },
};

const pausing: SandboxDetail = {
  ...sandboxFixture('pausing'),
  lifecycle: {
    ...sandboxFixture('pausing').lifecycle!,
    operation: {
      purpose: 'pause',
      phase: 'capture_and_publish_checkpoint',
      startedAt: '2026-09-08T00:00:00Z',
    },
  },
};

it('guards direct sends and approvals during an operator Pause without affecting retained history', async () => {
  const runAgent = vi.fn();
  const client = agentClient({ readSandbox: async () => pausing, runAgent });
  const { result } = renderHook(() => useAgentController(client, 'agt_1'));
  await waitFor(() => expect(result.current.inputAvailability.notice).toContain('Pausing'));
  await act(() => result.current.send('Do not queue during Pause'));
  await act(() => result.current.approve('approval-local', 'allow'));
  expect(runAgent).not.toHaveBeenCalled();
  expect(result.current.state).toMatchObject({
    status: 'ready',
    conversation: {
      messages: expect.arrayContaining([
        expect.objectContaining({ content: 'The repository is ready.' }),
      ]),
    },
  });
});

it('guards sends and approvals during capture, missing metadata and stale reads without dropping history', async () => {
  vi.useFakeTimers();
  const readSandbox = vi
    .fn()
    .mockResolvedValueOnce(saving)
    .mockRejectedValueOnce(new Error('offline'))
    .mockResolvedValueOnce({ ...sandboxFixture(), lifecycle: undefined })
    .mockResolvedValue(sandboxFixture());
  const runAgent = vi.fn().mockResolvedValue(undefined);
  const client = agentClient({ readSandbox, runAgent });
  const { result } = renderHook(() => useAgentController(client, 'agt_1'));
  await act(() => vi.advanceTimersByTimeAsync(0));
  for (const delay of [0, 1500, 1500]) {
    await act(() => vi.advanceTimersByTimeAsync(delay));
    await act(() => result.current.send('Keep this input'));
    await act(() => result.current.approve('approval-local', 'allow'));
    expect(runAgent).not.toHaveBeenCalled();
    expect(result.current.state).toMatchObject({
      status: 'ready',
      conversation: {
        messages: expect.arrayContaining([
          expect.objectContaining({ content: 'The repository is ready.' }),
        ]),
      },
    });
  }
  await act(() => vi.advanceTimersByTimeAsync(5000));
  await act(() => result.current.send('Explicit retry'));
  await act(() => result.current.approve('approval-local', 'allow'));
  expect(runAgent).toHaveBeenCalledTimes(2);
});

it('permits a normal paused message and never consults Pause eligibility for it', async () => {
  const runAgent = vi.fn().mockResolvedValue(undefined);
  const client = agentClient({
    readAgent: async () => ({ ...AGENT_DETAIL, status: 'paused' }),
    readSandbox: async () => sandboxFixture('paused'),
    runAgent,
  });
  const { result } = renderHook(() => useAgentController(client, 'agt_1'));
  await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
  await act(() => result.current.send('Continue from pause'));
  expect(runAgent).toHaveBeenCalledOnce();
});

it('does not interrupt or abort pending admission, then allows Stop after a real run starts', async () => {
  let onEvent: ((event: AgentLiveEvent) => void) | undefined;
  let runSignal: AbortSignal | undefined;
  const runAgent = vi.fn(
    (_request, options) =>
      new Promise<void>((_resolve, reject) => {
        onEvent = options.onEvent;
        runSignal = options.signal;
        options.signal.addEventListener('abort', () => reject(new Error('aborted')), {
          once: true,
        });
      }),
  );
  const interruptAgent = vi.fn().mockResolvedValue(undefined);
  const client = agentClient({ runAgent, interruptAgent });
  const { result } = renderHook(() => useAgentController(client, 'agt_1'));
  await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
  act(() => {
    void result.current.send('Await admission');
  });
  await waitFor(() => expect(result.current.pending).toBe('send'));
  await act(() => result.current.interrupt());
  expect(interruptAgent).not.toHaveBeenCalled();
  expect(runSignal?.aborted).toBe(false);
  act(() => onEvent?.({ type: 'RUN_STARTED', threadId: 'agt_1', runId: '5', cursor: 5 }));
  await act(() => result.current.interrupt());
  expect(interruptAgent).toHaveBeenCalledOnce();
  expect(runSignal?.aborted).toBe(true);
});

it.each(['saving', 'pausing', 'read_failed'] as const)(
  'keeps an actual ongoing turn interruptible when Sandbox observation is %s',
  async (mode) => {
    const interruptAgent = vi.fn().mockResolvedValue(undefined);
    const client = agentClient({
      readSandbox:
        mode !== 'read_failed'
          ? async () => (mode === 'saving' ? saving : pausing)
          : async () => {
              throw new Error('offline');
            },
      readAgentTranscript: async () => ({
        ...AGENT_TRANSCRIPT,
        turns: [{ ...AGENT_TRANSCRIPT.turns[0]!, outcome: { type: 'active' } }],
      }),
      interruptAgent,
    });
    const { result } = renderHook(() => useAgentController(client, 'agt_1'));
    await waitFor(() => expect(result.current.state.status).toBe('ready'));
    expect(result.current.inputAvailability.allowed).toBe(false);
    await act(() => result.current.interrupt());
    expect(interruptAgent).toHaveBeenCalledOnce();
  },
);

it('preserves the draft acceptance callback and explains a typed save race refusal without retrying', async () => {
  const accepted = vi.fn();
  const runAgent = vi
    .fn()
    .mockRejectedValue(
      new ApiError(409, 'Checkpoint input refusal', 'sandbox_checkpoint_in_progress'),
    );
  const client = agentClient({ runAgent });
  const { result } = renderHook(() => useAgentController(client, 'agt_1'));
  await waitFor(() => expect(result.current.inputAvailability.allowed).toBe(true));
  await act(() => result.current.send('Keep the draft', accepted));
  expect(accepted).not.toHaveBeenCalled();
  expect(runAgent).toHaveBeenCalledOnce();
  expect(result.current.actionError).toBe(
    'Progress is being saved. Your input was not accepted. Try again after saving finishes.',
  );
});
