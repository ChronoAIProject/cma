import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { ReactNode } from 'react';
import type { AgentProvenance } from '@cma/client';
import { CmaClientProvider } from '../api/context';
import { unavailableClient } from '../api/client';
import { useAgentProvenance } from './provenance';

const value: AgentProvenance = {
  agent_id: 'agt_one',
  created_at: '2026-09-06T00:00:00Z',
  creation_origin: 'manual',
  details: {
    created_by: { kind: 'human', subject: 'private-subject' },
    trigger: null,
    channel_bridge: null,
    workspace_profile_index: null,
  },
};

function deferred() {
  let resolve!: (value: AgentProvenance) => void;
  const promise = new Promise<AgentProvenance>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('clears privileged facts across client changes, including an identical read function', async () => {
  const pending = deferred();
  const readAgentProvenance = vi.fn().mockResolvedValueOnce(value).mockReturnValue(pending.promise);
  let client = { ...unavailableClient, readAgentProvenance };
  const wrapper = ({ children }: { children: ReactNode }) => (
    <CmaClientProvider client={client}>{children}</CmaClientProvider>
  );
  const { result, rerender } = renderHook(() => useAgentProvenance('agt_one', true), { wrapper });
  await waitFor(() => expect(result.current.status).toBe('ready'));
  client = { ...client };
  rerender();
  expect(result.current).toEqual({ status: 'loading', agentId: 'agt_one' });
  expect(readAgentProvenance.mock.calls[0]?.[1].signal.aborted).toBe(true);
  await act(async () => pending.resolve({ ...value, details: undefined }));
  expect(result.current.status).toBe('ready');
  expect(result.current.status === 'ready' && result.current.value.details).toBeUndefined();
});

it('does not restore facts on re-enable before a fresh authorized read completes', async () => {
  const pending = deferred();
  const readAgentProvenance = vi.fn().mockResolvedValueOnce(value).mockReturnValue(pending.promise);
  const client = { ...unavailableClient, readAgentProvenance };
  const wrapper = ({ children }: { children: ReactNode }) => (
    <CmaClientProvider client={client}>{children}</CmaClientProvider>
  );
  const { result, rerender } = renderHook(({ enabled }) => useAgentProvenance('agt_one', enabled), {
    initialProps: { enabled: true },
    wrapper,
  });
  await waitFor(() => expect(result.current.status).toBe('ready'));
  rerender({ enabled: false });
  expect(result.current).not.toHaveProperty('value');
  rerender({ enabled: true });
  expect(result.current.status).toBe('loading');
  await act(async () => pending.resolve({ ...value, details: undefined }));
  expect(result.current.status === 'ready' && result.current.value.details).toBeUndefined();
});

it('refuses a mismatched reply and ignores a late reply for a previous Agent', async () => {
  const pending = deferred();
  const readAgentProvenance = vi.fn().mockReturnValueOnce(pending.promise).mockResolvedValue(value);
  const client = { ...unavailableClient, readAgentProvenance };
  const wrapper = ({ children }: { children: ReactNode }) => (
    <CmaClientProvider client={client}>{children}</CmaClientProvider>
  );
  const { result, rerender } = renderHook(({ id }) => useAgentProvenance(id, true), {
    initialProps: { id: 'agt_one' },
    wrapper,
  });
  rerender({ id: 'agt_two' });
  await waitFor(() => expect(result.current.status).toBe('unavailable'));
  await act(async () => pending.resolve(value));
  expect(result.current).toEqual({ status: 'unavailable', agentId: 'agt_two' });
});

it('polls without overlap, clears facts after refusal and cancels on unmount', async () => {
  vi.useFakeTimers();
  const pending = deferred();
  const readAgentProvenance = vi
    .fn()
    .mockResolvedValueOnce(value)
    .mockRejectedValueOnce(new Error('Denied'))
    .mockReturnValue(pending.promise);
  const client = { ...unavailableClient, readAgentProvenance };
  const wrapper = ({ children }: { children: ReactNode }) => (
    <CmaClientProvider client={client}>{children}</CmaClientProvider>
  );
  const { result, unmount } = renderHook(() => useAgentProvenance('agt_one', true), { wrapper });
  await act(async () => {});
  expect(result.current.status).toBe('ready');
  await act(async () => vi.advanceTimersByTimeAsync(60_000));
  expect(result.current).toEqual({ status: 'unavailable', agentId: 'agt_one' });
  await act(async () => document.dispatchEvent(new Event('visibilitychange')));
  await act(async () => vi.advanceTimersByTimeAsync(120_000));
  expect(readAgentProvenance).toHaveBeenCalledTimes(3);
  unmount();
  expect(readAgentProvenance.mock.calls[2]?.[1].signal.aborted).toBe(true);
  await act(async () => pending.resolve(value));
  await act(async () => vi.advanceTimersByTimeAsync(60_000));
  expect(readAgentProvenance).toHaveBeenCalledTimes(3);
});

it('waits until the page is visible to fetch optional facts', async () => {
  const visibility = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden');
  const readAgentProvenance = vi.fn().mockResolvedValue(value);
  const client = { ...unavailableClient, readAgentProvenance };
  const wrapper = ({ children }: { children: ReactNode }) => (
    <CmaClientProvider client={client}>{children}</CmaClientProvider>
  );
  const { result } = renderHook(() => useAgentProvenance('agt_one', true), { wrapper });
  expect(readAgentProvenance).not.toHaveBeenCalled();
  visibility.mockReturnValue('visible');
  await act(async () => document.dispatchEvent(new Event('visibilitychange')));
  expect(result.current.status).toBe('ready');
});
