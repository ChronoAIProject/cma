import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ApiError } from '@chronoai/cma-sdk';
import { useAgentController } from './controller';
import { agentClient, AGENT_DETAIL, AGENT_USAGE } from '../test/agent';

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});
it.each(['detail', 'usage'] as const)(
  'automatically recovers %s 429 after Retry-After without hidden requests',
  async (route) => {
    vi.useFakeTimers();
    vi.spyOn(Math, 'random').mockReturnValue(0);
    const hidden = vi.spyOn(document, 'hidden', 'get').mockReturnValue(false);
    const read = vi
      .fn()
      .mockRejectedValueOnce(
        new ApiError(429, 'Busy', 'buffered_read_capacity_exhausted', undefined, 20_000),
      )
      .mockResolvedValue(route === 'detail' ? AGENT_DETAIL : AGENT_USAGE);
    const client = agentClient(route === 'detail' ? { readAgent: read } : { readAgentUsage: read });
    const hook = renderHook(() => useAgentController(client, 'agt_1'));
    await act(async () => {});
    expect(read).toHaveBeenCalledTimes(1);
    if (route === 'detail')
      expect(hook.result.current.state).toMatchObject({
        status: 'error',
        retryAt: Date.now() + 20_000,
      });
    const beforeReload = hook.result.current.state;
    act(() => {
      hook.result.current.reload();
      hook.result.current.reload();
    });
    expect(hook.result.current.state).toBe(beforeReload);
    await act(() => vi.advanceTimersByTimeAsync(19_999));
    expect(read).toHaveBeenCalledTimes(1);
    hidden.mockReturnValue(true);
    act(() => document.dispatchEvent(new Event('visibilitychange')));
    await act(() => vi.advanceTimersByTimeAsync(60_000));
    expect(read).toHaveBeenCalledTimes(1);
    hidden.mockReturnValue(false);
    act(() => document.dispatchEvent(new Event('visibilitychange')));
    await act(() => vi.advanceTimersByTimeAsync(0));
    expect(read).toHaveBeenCalledTimes(2);
    expect(hook.result.current.state.status).toBe('ready');
    hook.unmount();
  },
);

it.each([45_000, 90_000])(
  'retains a %ims read floor through an exact-scope remount',
  async (delay) => {
    vi.useFakeTimers();
    vi.spyOn(Math, 'random').mockReturnValue(0);
    const read = vi
      .fn()
      .mockRejectedValueOnce(new ApiError(429, 'Busy', 'rate_limited', undefined, delay))
      .mockResolvedValue(AGENT_DETAIL);
    const client = agentClient({ readAgent: read });
    const mount = () =>
      renderHook(() => useAgentController(client, 'agt_1', { viewerId: 'alice' }));
    let view = mount();
    await act(async () => {});
    view.unmount();
    await act(() => vi.advanceTimersByTimeAsync(1000));
    view = mount();
    act(() => {
      view.result.current.reload();
      window.dispatchEvent(new Event('focus'));
    });
    await act(() => vi.advanceTimersByTimeAsync(delay - 1001));
    expect(read).toHaveBeenCalledOnce();
    await act(() => vi.advanceTimersByTimeAsync(1));
    expect(read).toHaveBeenCalledTimes(2);
    expect(view.result.current.state.status).toBe('ready');
    view.unmount();
  },
);
