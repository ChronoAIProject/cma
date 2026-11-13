import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { useAgentController } from '../controller';
import { harness } from './controller.fixture';
import { agentTransportJournal, clearAgentTransport } from './journal';
import { useOpeningPreparation } from '../useOpeningPreparation';
import { AGENT_DETAIL } from '../../test/agent';

beforeEach(() => {
  sessionStorage.clear();
  vi.useFakeTimers();
  vi.spyOn(Math, 'random').mockReturnValue(0);
});
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it.each([
  [45, false],
  [90, false],
  [45, true],
  [90, true],
] as const)(
  'retains the %is eligibility floor across reload/focus/remount with route preparation=%s',
  async (seconds, prepare) => {
    const h = harness();
    h.eligibility.mockImplementationOnce(
      async () =>
        new Response(
          JSON.stringify({ status: 429, code: 'temporarily_busy', detail: 'Try later.' }),
          {
            status: 429,
            headers: { 'content-type': 'application/problem+json', 'retry-after': String(seconds) },
          },
        ),
    );
    const mount = () =>
      renderHook(() => {
        useOpeningPreparation(h.client, prepare ? 'agt_1' : undefined, 'alice', true, false);
        return useAgentController(h.client, 'agt_1', { viewerId: 'alice' });
      });
    let view = mount();
    await act(async () => {});
    expect(view.result.current.state.status).toBe('error');
    expect(h.eligibility).toHaveBeenCalledOnce();
    expect(agentTransportJournal(sessionStorage, 'alice', 'agt_1').selected()).toBeUndefined();
    act(() => {
      view.result.current.reload();
      view.result.current.reload();
      window.dispatchEvent(new Event('focus'));
    });
    await act(async () => {});
    expect(h.eligibility).toHaveBeenCalledOnce();
    view.unmount();
    await act(() => vi.advanceTimersByTimeAsync(1000));
    view = mount();
    await act(async () => {});
    expect(h.eligibility).toHaveBeenCalledOnce();
    await act(() => vi.advanceTimersByTimeAsync(seconds * 1000 - 1001));
    expect(h.eligibility).toHaveBeenCalledOnce();
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(h.eligibility).toHaveBeenCalledTimes(2);
    expect(view.result.current.state.status).toBe('ready');
    expect(h.post).not.toHaveBeenCalled();
    view.unmount();
  },
);

it.each(['alice', 'bob'])('cancels an abandoned retry after a new %s sign-in', async (viewer) => {
  const h = harness();
  h.eligibility.mockImplementationOnce(
    async () =>
      new Response(JSON.stringify({ status: 429, code: 'busy', detail: 'Try later.' }), {
        status: 429,
        headers: { 'content-type': 'application/problem+json', 'retry-after': '90' },
      }),
  );
  const first = renderHook(() => useAgentController(h.client, 'agt_1', { viewerId: 'alice' }));
  await act(async () => {});
  expect(first.result.current.state.status).toBe('error');
  first.unmount();
  clearAgentTransport(sessionStorage);
  h.currentUser.mockResolvedValue(viewer);
  const second = renderHook(() => useAgentController(h.client, 'agt_1', { viewerId: viewer }));
  await act(async () => {});
  expect(h.eligibility).toHaveBeenCalledTimes(2);
  expect(second.result.current.state.status).toBe('ready');
  second.unmount();
  await act(() => vi.advanceTimersByTimeAsync(90_000));
  expect(h.eligibility).toHaveBeenCalledTimes(2);
  expect(h.post).not.toHaveBeenCalled();
});

it('aborts prepared companion reads when selection fails in the real controller', async () => {
  const h = harness();
  let refuse!: (value: Response) => void;
  const eligibility = new Promise<Response>((resolve) => (refuse = resolve));
  h.eligibility.mockReturnValue(eligibility);
  const transcript = vi.spyOn(h.client, 'readAgentTranscript').mockImplementation(
    (_agent, options) =>
      new Promise((_resolve, reject) => {
        options?.signal?.addEventListener('abort', () => reject(options.signal?.reason), {
          once: true,
        });
      }),
  );
  const lineage = {
    agentId: AGENT_DETAIL.id,
    workspaceId: AGENT_DETAIL.workspace.id,
    sandboxId: AGENT_DETAIL.sandbox.id,
  };
  const view = renderHook(() => {
    useOpeningPreparation(h.client, 'agt_1', 'alice', true, false, lineage);
    return useAgentController(h.client, 'agt_1', {
      viewerId: 'alice',
      authorizedLineage: lineage,
    });
  });
  await act(async () => {});
  expect(transcript).toHaveBeenCalledOnce();
  const signal = transcript.mock.calls[0]![1]!.signal!;
  expect(signal.aborted).toBe(false);
  await act(async () => {
    refuse(
      new Response(JSON.stringify({ status: 429, code: 'busy', detail: 'Try later.' }), {
        status: 429,
        headers: { 'content-type': 'application/problem+json', 'retry-after': '45' },
      }),
    );
  });
  expect(view.result.current.state.status).toBe('error');
  expect(signal.aborted).toBe(true);
  expect(h.snapshot).not.toHaveBeenCalled();
  expect(h.post).not.toHaveBeenCalled();
  view.unmount();
});
