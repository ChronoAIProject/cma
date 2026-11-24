import { act, renderHook, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import type { ReplyPreview } from '../api/replyPreview';
import type { CmaWebClient } from '../api/client';
import type { NavigationState } from '../shell/navigation';
import { navigationFixture } from '../test/hierarchy';
import { testClient } from '../test/client';
import { useCrewReplyPreviews } from './useCrewReplyPreviews';

afterEach(() => vi.restoreAllMocks());

const ready = (agentId: string): ReplyPreview => ({
  kind: 'ready',
  agentId,
  sourceThroughSeq: 9,
  reply: {
    text: `Reply from ${agentId}`,
    truncated: false,
    messageSequence: 8,
    turnSequence: 2,
    appendedAt: '2026-11-23T10:30:00Z',
  },
});
const navigation = (): NavigationState => ({
  status: 'ready',
  items: navigationFixture('active'),
});
function pendingClient() {
  const calls: {
    id: string;
    signal?: AbortSignal;
    resolve: (value: ReplyPreview) => void;
    reject: (cause: unknown) => void;
  }[] = [];
  const readAgentReplyPreview = vi.fn<NonNullable<CmaWebClient['readAgentReplyPreview']>>(
    (id, options) =>
      new Promise((resolve, reject) => {
        calls.push({ id, signal: options?.signal, resolve, reject });
      }),
  );
  return { client: testClient({ readAgentReplyPreview }), calls, readAgentReplyPreview };
}
const props = (client: CmaWebClient) => ({
  client,
  viewerId: 'viewer-one',
  authEpoch: 1,
  scope: '',
  navigation: navigation(),
  visibleAgentIds: ['agent-one', 'agent-two', 'agent-three', 'agent-four'],
  enabled: true,
});

it('reads at most three distinct cards with two in flight and isolates one failed read', async () => {
  const test = pendingClient();
  const initial = props(test.client);
  const { result, rerender } = renderHook(useCrewReplyPreviews, { initialProps: initial });
  expect(test.calls.map(({ id }) => id)).toEqual(['agent-one', 'agent-two']);
  expect(result.current).toEqual({});
  await act(async () => test.calls[0]!.reject(new Error('History unavailable')));
  expect(test.calls.map(({ id }) => id)).toEqual(['agent-one', 'agent-two', 'agent-three']);
  await act(async () => {
    test.calls[1]!.resolve(ready('agent-two'));
    test.calls[2]!.resolve(ready('agent-three'));
  });
  expect(Object.keys(result.current)).toEqual(['agent-two', 'agent-three']);
  rerender({
    ...initial,
    navigation: { ...initial.navigation },
    visibleAgentIds: [...initial.visibleAgentIds],
  });
  expect(test.calls).toHaveLength(3);
});

it('deduplicates IDs and never creates an independent refresh timer', async () => {
  const readAgentReplyPreview = vi.fn(async (id: string) => ready(id));
  const initial = props(testClient({ readAgentReplyPreview }));
  initial.visibleAgentIds = ['agent-one', 'agent-one', 'agent-two'];
  const { result, rerender } = renderHook(useCrewReplyPreviews, { initialProps: initial });
  await waitFor(() => expect(Object.keys(result.current)).toHaveLength(2));
  expect(readAgentReplyPreview).toHaveBeenCalledTimes(2);
  vi.useFakeTimers();
  try {
    await act(async () => vi.advanceTimersByTime(120_000));
    expect(readAgentReplyPreview).toHaveBeenCalledTimes(2);
    rerender({ ...initial, navigation: navigation() });
    await act(async () => {});
    expect(readAgentReplyPreview).toHaveBeenCalledTimes(4);
  } finally {
    vi.useRealTimers();
  }
});

it.each(['viewer', 'client', 'auth', 'cards', 'refresh'] as const)(
  'cancels stale %s reads and rejects even a transport that ignores cancellation',
  async (change) => {
    const test = pendingClient();
    const next = pendingClient();
    const initial = props(test.client);
    const { result, rerender } = renderHook(useCrewReplyPreviews, { initialProps: initial });
    const updated = {
      ...initial,
      ...(change === 'viewer' ? { viewerId: 'viewer-two' } : {}),
      ...(change === 'client' ? { client: next.client } : {}),
      ...(change === 'auth' ? { authEpoch: 2 } : {}),
      ...(change === 'cards' ? { visibleAgentIds: ['agent-five'] } : {}),
      ...(change === 'refresh' ? { navigation: navigation() } : {}),
    };
    rerender(updated);
    expect(test.calls[0]!.signal!.aborted).toBe(true);
    expect(test.calls[1]!.signal!.aborted).toBe(true);
    await act(async () => {
      test.calls[0]!.resolve(ready('agent-one'));
      test.calls[1]!.resolve(ready('agent-two'));
    });
    expect(result.current).toEqual({});
    const fresh = change === 'client' ? next.calls[0]! : test.calls[2]!;
    await act(async () => fresh.resolve(ready(fresh.id)));
    expect(Object.keys(result.current)).toEqual([fresh.id]);
  },
);

it.each(['shared', 'disabled', 'loading', 'error', 'stale', 'signed-out'] as const)(
  'clears replies immediately for %s and stops remaining queued reads',
  async (condition) => {
    const test = pendingClient();
    const initial = props(test.client);
    const { result, rerender } = renderHook(useCrewReplyPreviews, { initialProps: initial });
    await act(async () => test.calls[0]!.resolve(ready('agent-one')));
    expect(result.current['agent-one']).toBeDefined();
    rerender({
      ...initial,
      ...(condition === 'shared' ? { scope: 'organization-one' } : {}),
      ...(condition === 'disabled' ? { enabled: false } : {}),
      ...(condition === 'signed-out' ? { viewerId: '' } : {}),
      ...(condition === 'loading' ? { navigation: { status: 'loading' } as NavigationState } : {}),
      ...(condition === 'error'
        ? { navigation: { status: 'error', message: 'Forbidden' } as NavigationState }
        : {}),
      ...(condition === 'stale'
        ? {
            navigation: {
              status: 'ready',
              items: navigationFixture('active'),
              refreshError: 'Could not refresh',
            } as NavigationState,
          }
        : {}),
    });
    expect(result.current).toEqual({});
    expect(test.calls.every(({ signal }) => signal?.aborted)).toBe(true);
    await act(async () => test.calls[1]!.resolve(ready('agent-two')));
    expect(result.current).toEqual({});
  },
);

it('aborts hidden reads and waits for a fresh navigation observation after visibility resumes', async () => {
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(false);
  const test = pendingClient();
  const initial = props(test.client);
  const { result, rerender } = renderHook(useCrewReplyPreviews, { initialProps: initial });
  await act(async () => test.calls[0]!.resolve(ready('agent-one')));
  hidden.mockReturnValue(true);
  act(() => document.dispatchEvent(new Event('visibilitychange')));
  expect(result.current).toEqual({});
  expect(test.calls.every(({ signal }) => signal?.aborted)).toBe(true);
  await act(async () => test.calls[1]!.resolve(ready('agent-two')));
  hidden.mockReturnValue(false);
  act(() => document.dispatchEvent(new Event('visibilitychange')));
  expect(test.calls).toHaveLength(3);
  expect(result.current).toEqual({});
  rerender({ ...initial, navigation: navigation() });
  expect(test.calls).toHaveLength(5);
});

it('does not start reads while hidden and aborts both workers on unmount', () => {
  const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(true);
  const test = pendingClient();
  const initial = props(test.client);
  const { rerender, unmount } = renderHook(useCrewReplyPreviews, { initialProps: initial });
  expect(test.calls).toHaveLength(0);
  hidden.mockReturnValue(false);
  rerender({ ...initial, navigation: navigation() });
  expect(test.calls).toHaveLength(2);
  unmount();
  expect(test.calls.every(({ signal }) => signal?.aborted)).toBe(true);
});

it('ignores navigation refreshes while the host is hidden and resumes within its original budget', async () => {
  const test = pendingClient();
  const initial = { ...props(test.client), visible: true };
  const { result, rerender } = renderHook(useCrewReplyPreviews, { initialProps: initial });
  expect(test.calls).toHaveLength(2);
  rerender({ ...initial, visible: false });
  expect(test.calls.every(({ signal }) => signal?.aborted)).toBe(true);
  const refreshed = { ...initial, navigation: navigation() };
  rerender({ ...refreshed, visible: false });
  await act(async () => {
    test.calls[0]!.resolve(ready('agent-one'));
    test.calls[1]!.resolve(ready('agent-two'));
  });
  expect(result.current).toEqual({});
  expect(test.calls).toHaveLength(2);
  rerender(refreshed);
  expect(test.calls).toHaveLength(4);
  await act(async () => {
    test.calls[2]!.resolve(ready('agent-one'));
    test.calls[3]!.resolve(ready('agent-two'));
  });
  expect(test.calls).toHaveLength(5);
  await act(async () => test.calls[4]!.resolve(ready('agent-three')));
  expect(Object.keys(result.current)).toEqual(['agent-one', 'agent-two', 'agent-three']);
});

it('leaves unavailable, absent and mismatched replies blank', async () => {
  const test = pendingClient();
  const { result } = renderHook(useCrewReplyPreviews, { initialProps: props(test.client) });
  await act(async () => {
    test.calls[0]!.resolve({ kind: 'none', agentId: 'agent-one', sourceThroughSeq: 0 });
    test.calls[1]!.resolve({ kind: 'unavailable', agentId: 'agent-two', sourceThroughSeq: null });
  });
  await act(async () => test.calls[2]!.resolve(ready('wrong-agent')));
  expect(result.current).toEqual({});
});
