import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AGENT_TRANSCRIPT, agentClient } from '../test/agent';
import { downloadAgentHistory } from './historyExport';
import { useHistoryExport } from './useHistoryExport';

vi.mock('./historyExport', () => ({ downloadAgentHistory: vi.fn() }));
afterEach(() => {
  vi.clearAllMocks();
  vi.useRealTimers();
});

it('exports only the bounded no-store transcript for a retired Agent', async () => {
  const client = agentClient();
  const read = vi.spyOn(client, 'readAgentTranscript');
  const events = vi.spyOn(client, 'readAgentEvents');
  const raw = vi.spyOn(client, 'exportAgentHistory');
  const hook = renderHook(() => useHistoryExport(client, 'owner', true));
  await act(() => hook.result.current.run('agt_1', true, 'transcript'));
  expect(read).toHaveBeenCalledExactlyOnceWith('agt_1', {
    signal: expect.any(AbortSignal),
    cache: 'no-store',
    maxPages: 256,
    maxBytes: 16 * 1024 * 1024,
  });
  expect(raw).not.toHaveBeenCalled();
  expect(events).not.toHaveBeenCalled();
  expect(downloadAgentHistory).toHaveBeenCalledExactlyOnceWith('agt_1', {
    transcript: AGENT_TRANSCRIPT,
  });
  expect(hook.result.current.notice).toContain('recorded messages and turn outcomes');
});

it.each(['client', 'viewerId', 'enabled', 'authority'] as const)(
  'refuses captured export and late data after %s changes',
  async (field) => {
    let complete!: (value: typeof AGENT_TRANSCRIPT) => void;
    let signal: AbortSignal | undefined;
    const read = vi.fn((_id, options) => {
      signal = options.signal;
      return new Promise<typeof AGENT_TRANSCRIPT>((resolve) => {
        complete = resolve;
      });
    });
    const client = agentClient({ readAgentTranscript: read });
    const initialProps = { client, viewerId: 'owner', enabled: true, authority: {} };
    const hook = renderHook(
      ({ client, viewerId, enabled, authority }) =>
        useHistoryExport(client, viewerId, enabled, authority),
      { initialProps },
    );
    const captured = hook.result.current.run;
    let pending!: Promise<void>;
    act(() => {
      pending = captured('agt_1', true);
    });
    await waitFor(() => expect(read).toHaveBeenCalledOnce());
    hook.rerender({
      ...initialProps,
      [field]: {
        client: agentClient(),
        viewerId: 'another-owner',
        enabled: false,
        authority: {},
      }[field],
    });
    expect(signal?.aborted).toBe(true);
    await act(() => pending);
    await act(() => captured('agt_1', true));
    expect(read).toHaveBeenCalledOnce();
    await act(async () => complete(AGENT_TRANSCRIPT));
    expect(downloadAgentHistory).not.toHaveBeenCalled();
    expect(hook.result.current).toMatchObject({
      pending: false,
      error: undefined,
      notice: undefined,
    });
  },
);

it('refuses a captured export after unmount without starting another read', async () => {
  const client = agentClient();
  const read = vi.spyOn(client, 'readAgentTranscript');
  const hook = renderHook(() => useHistoryExport(client, 'owner', true));
  const captured = hook.result.current.run;
  hook.unmount();
  await captured('agt_1', true);
  expect(read).not.toHaveBeenCalled();
});

it('bounds a transport that ignores cancellation and never downloads its late data', async () => {
  vi.useFakeTimers();
  let complete!: (value: typeof AGENT_TRANSCRIPT) => void;
  const client = agentClient({
    readAgentTranscript: () =>
      new Promise((resolve) => {
        complete = resolve;
      }),
  });
  const hook = renderHook(() => useHistoryExport(client, 'owner', true));
  let pending!: Promise<void>;
  act(() => {
    pending = hook.result.current.run('agt_1', true);
  });
  await act(() => vi.advanceTimersByTimeAsync(120_000));
  await act(() => pending);
  expect(hook.result.current).toMatchObject({
    pending: false,
    error: 'History export timed out. Please retry.',
  });
  await act(async () => complete(AGENT_TRANSCRIPT));
  expect(downloadAgentHistory).not.toHaveBeenCalled();
});

it('does not download a partial or wrong-Agent export', async () => {
  const read = vi
    .fn()
    .mockRejectedValueOnce(new Error('Transcript snapshot changed.'))
    .mockResolvedValueOnce({ ...AGENT_TRANSCRIPT, agentId: 'other' });
  const client = agentClient({ readAgentTranscript: read });
  const hook = renderHook(() => useHistoryExport(client, 'owner', true));
  await act(() => hook.result.current.run('agt_1', true));
  expect(hook.result.current.error).toContain('snapshot changed');
  await act(() => hook.result.current.run('agt_1', true));
  expect(hook.result.current.error).toContain('did not match');
  expect(downloadAgentHistory).not.toHaveBeenCalled();
});
