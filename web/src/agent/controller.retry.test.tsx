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
