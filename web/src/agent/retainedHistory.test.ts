import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
import { AGENT_DETAIL, AGENT_TRANSCRIPT, agentClient } from '../test/agent';
import type { RetiredAgentPresentation } from './retiredPresentation';
import { loadRetainedHistory, useRetainedHistory } from './retainedHistory';
import { conversationFromTranscript } from './conversation';

const presentation: RetiredAgentPresentation = {
  kind: 'retired',
  detail: { ...AGENT_DETAIL, sandbox: { ...AGENT_DETAIL.sandbox, status: 'retired' } },
  identity: {
    viewerId: 'owner',
    authEpoch: 1,
    agentId: 'agt_1',
    worktreeId: 'wkt_1',
    workspaceId: 'wsp_1',
    sandboxId: 'sbx_1',
  },
};
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('reads bounded retained history without opening runtime, Workspace, usage or events', async () => {
  const client = agentClient();
  const transcript = vi.spyOn(client, 'readAgentTranscript');
  const forbidden = [
    'readWorkspace',
    'readAgentUsage',
    'readAgentEvents',
    'replayAgent',
    'runAgent',
  ] as const;
  const spies = forbidden.map((name) => vi.spyOn(client, name));
  const result = await loadRetainedHistory(
    client,
    presentation.identity,
    new AbortController().signal,
  );
  expect(result.messages.map(({ content }) => content)).toEqual([
    'Inspect this repository.',
    'The repository is ready.',
  ]);
  expect(transcript).toHaveBeenCalledExactlyOnceWith(
    'agt_1',
    expect.objectContaining({
      cache: 'no-store',
      maxPages: 256,
      maxBytes: 16 * 1024 * 1024,
    }),
  );
  for (const spy of spies) expect(spy).not.toHaveBeenCalled();
});

it('rejects a different Agent projection without publishing it', async () => {
  const client = agentClient({
    readAgentTranscript: async () => ({ ...AGENT_TRANSCRIPT, agentId: 'other' }),
  });
  await expect(
    loadRetainedHistory(client, presentation.identity, new AbortController().signal),
  ).rejects.toThrow('did not match');
});

it('preserves independent failed and interrupted turn outcomes without inferring a deletion cause', async () => {
  const original = AGENT_TRANSCRIPT.turns[0]!;
  const client = agentClient({
    readAgentTranscript: async () => ({
      ...AGENT_TRANSCRIPT,
      turns: [
        { ...original, outcome: { type: 'failed', failure: { class: 'synthetic' } } },
        { ...original, turnSequence: 5, messages: [], outcome: { type: 'interrupted' } },
      ],
    }),
  });
  const result = await loadRetainedHistory(
    client,
    presentation.identity,
    new AbortController().signal,
  );
  expect(result.turnOutcomes).toEqual({ '1': 'failed', '5': 'interrupted' });
  expect(result.recovery).toEqual([
    expect.objectContaining({ runId: '1', title: 'Agent run failed' }),
  ]);
});

it('keeps recorded failure diagnostics without offering a message to a retired Agent', async () => {
  const transcript = {
    ...AGENT_TRANSCRIPT,
    turns: [
      {
        ...AGENT_TRANSCRIPT.turns[0]!,
        outcome: { type: 'failed', failure: { class: 'upstream_unavailable' } },
      },
    ],
  };
  const original = structuredClone(transcript);
  const client = agentClient({ readAgentTranscript: async () => transcript });
  const result = await loadRetainedHistory(
    client,
    presentation.identity,
    new AbortController().signal,
  );
  expect(result.recovery[0]).toMatchObject({
    title: 'Agent run failed',
    detail: 'This recorded Agent run failed. Its delivery or task effects may be incomplete.',
    technicalDetails: { category: 'upstream_unavailable', httpStatus: 503 },
  });
  expect(result.recovery[0]?.nextStep).toBeUndefined();
  expect(result.turnOutcomes).toEqual({ '1': 'failed' });
  expect(transcript).toEqual(original);
  expect(conversationFromTranscript(transcript).recovery[0]?.nextStep).toContain(
    'A new message continues this Agent conversation',
  );
});

it('drops an old read immediately on authority loss even if the transport ignores cancellation', async () => {
  let resolve!: (value: typeof AGENT_TRANSCRIPT) => void;
  let signal: AbortSignal | undefined;
  const client = agentClient({
    readAgentTranscript: (_id, options) => {
      signal = options?.signal;
      return new Promise((done) => {
        resolve = done;
      });
    },
  });
  const initialProps: { current?: RetiredAgentPresentation } = { current: presentation };
  const hook = renderHook(
    ({ current }: { current?: RetiredAgentPresentation }) => useRetainedHistory(client, current),
    {
      initialProps,
    },
  );
  await waitFor(() => expect(signal).toBeDefined());
  hook.rerender({ current: undefined });
  expect(hook.result.current.status).toBe('unconfirmed');
  expect(signal?.aborted).toBe(true);
  await act(async () => resolve(AGENT_TRANSCRIPT));
  expect(hook.result.current.status).toBe('unconfirmed');
});

it.each(['viewerId', 'authEpoch', 'agentId'] as const)(
  'clears visible history and fences captured retry after %s changes',
  async (field) => {
    const read = vi
      .fn()
      .mockResolvedValueOnce(AGENT_TRANSCRIPT)
      .mockImplementation(() => new Promise(() => {}));
    const client = agentClient({ readAgentTranscript: read });
    const hook = renderHook(({ current }) => useRetainedHistory(client, current), {
      initialProps: { current: presentation },
    });
    await waitFor(() => expect(hook.result.current.status).toBe('ready'));
    const previous = hook.result.current;
    hook.rerender({
      current: {
        ...presentation,
        identity: { ...presentation.identity, [field]: field === 'authEpoch' ? 2 : 'other' },
      },
    });
    expect(hook.result.current.status).toBe('loading');
    expect(hook.result.current).not.toHaveProperty('conversation');
    act(() => previous.retry());
    await waitFor(() => expect(read).toHaveBeenCalledTimes(2));
  },
);

it('preserves the server retry floor and never automatically retries the retained read', async () => {
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
  const read = vi
    .fn()
    .mockRejectedValueOnce(new ApiError(429, 'Busy', undefined, undefined, 20_000))
    .mockResolvedValue(AGENT_TRANSCRIPT);
  const client = agentClient({ readAgentTranscript: read });
  const hook = renderHook(() => useRetainedHistory(client, presentation));
  await act(async () => {});
  expect(hook.result.current.status).toBe('error');
  act(() => hook.result.current.retry());
  expect(read).toHaveBeenCalledTimes(1);
  await act(() => vi.advanceTimersByTimeAsync(20_000));
  expect(read).toHaveBeenCalledTimes(1);
  act(() => {
    hook.result.current.retry();
    hook.result.current.retry();
  });
  await act(async () => {});
  expect(read).toHaveBeenCalledTimes(2);
  expect(hook.result.current.status).toBe('ready');
});

it('times out a stalled read and preserves a separate explicit history retry', async () => {
  vi.useFakeTimers();
  const read = vi.fn().mockImplementation(() => new Promise(() => {}));
  const client = agentClient({ readAgentTranscript: read });
  const hook = renderHook(() => useRetainedHistory(client, presentation));
  await act(() => vi.advanceTimersByTimeAsync(120_000));
  expect(hook.result.current).toMatchObject({
    status: 'error',
    message: 'The retained conversation read timed out.',
  });
  await act(() => vi.advanceTimersByTimeAsync(120_000));
  expect(read).toHaveBeenCalledTimes(1);
});
