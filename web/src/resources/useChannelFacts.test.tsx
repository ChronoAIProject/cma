import { act, renderHook } from '@testing-library/react';
import { createClient } from '@chronoai/cma-sdk';
import { afterEach, expect, it, vi } from 'vitest';
import { useChannelFacts } from './useChannelFacts';

afterEach(() => vi.useRealTimers());
const summary = { active: false, has_history: true, first_used_at: null, last_used_at: null };

it('retains successful summaries when a sibling stalls and ignores its late reply', async () => {
  vi.useFakeTimers();
  let finish: ((value: Response) => void) | undefined;
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input) => {
      const id = new URL(String(input)).pathname.split('/')[4]!;
      return id === 'agt_slow'
        ? new Promise<Response>((resolve) => {
            finish = resolve;
          })
        : Response.json({ agent_id: id, channel_usage: summary });
    },
  });
  const { result } = renderHook(() => useChannelFacts(client, ['agt_one', 'agt_slow', 'agt_two']));
  await act(async () => vi.advanceTimersByTimeAsync(100));
  expect(result.current.size).toBe(0);
  expect(finish).toBeDefined();
  await act(async () => vi.advanceTimersByTimeAsync(3_000));
  expect(result.current.get('agt_one')).toEqual(summary);
  expect(result.current.get('agt_two')).toEqual(summary);
  expect(result.current.get('agt_slow')).toBeUndefined();
  await act(async () =>
    finish?.(Response.json({ agent_id: 'agt_slow', channel_usage: { ...summary, active: true } })),
  );
  expect(result.current.get('agt_slow')).toBeUndefined();
  expect(result.current.get('agt_one')).toEqual(summary);
});

it('stops a slow batch before the view timeout with four requests at most and leaves unvisited rows unknown', async () => {
  vi.useFakeTimers();
  const requested: string[] = [];
  let live = 0;
  let maximum = 0;
  const ids = Array.from({ length: 40 }, (_, index) => `agt_${String(index).padStart(2, '0')}`);
  const client = createClient({
    baseUrl: 'https://fixture.invalid',
    fetch: async (input, init) => {
      const id = new URL(String(input)).pathname.split('/')[4]!;
      requested.push(id);
      if (id === ids[0]) return Response.json({ agent_id: id, channel_usage: summary });
      live += 1;
      maximum = Math.max(maximum, live);
      return new Promise<Response>((_, reject) => {
        init?.signal?.addEventListener(
          'abort',
          () => {
            live -= 1;
            reject(new DOMException('Aborted', 'AbortError'));
          },
          { once: true },
        );
      });
    },
  });
  const { result } = renderHook(() => useChannelFacts(client, ids));
  await act(async () => vi.advanceTimersByTimeAsync(12_100));
  expect(result.current.get(ids[0]!)).toEqual(summary);
  expect(result.current.get(ids.at(-1)!)).toBeUndefined();
  expect(requested).not.toContain(ids.at(-1));
  expect(maximum).toBe(4);
  expect(live).toBe(0);
  const stoppedAt = requested.length;
  await act(async () => vi.advanceTimersByTimeAsync(3_000));
  expect(requested).toHaveLength(stoppedAt);
  expect(result.current.get(ids[0]!)).toEqual(summary);
});

it('makes no hidden-tree requests and refreshes immediately when the tree becomes active', async () => {
  vi.useFakeTimers();
  const fetch = vi.fn(async () => Response.json({ agent_id: 'agt_one', channel_usage: summary }));
  const client = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const { result, rerender } = renderHook(
    ({ active }) => useChannelFacts(client, ['agt_one'], active, 'viewer'),
    { initialProps: { active: false } },
  );
  await act(() => vi.advanceTimersByTimeAsync(60000));
  expect(fetch).not.toHaveBeenCalled();
  rerender({ active: true });
  await act(async () => {});
  expect(fetch).toHaveBeenCalledOnce();
  expect(result.current.get('agt_one')).toEqual(summary);
  rerender({ active: false });
  await act(() => vi.advanceTimersByTimeAsync(60000));
  expect(fetch).toHaveBeenCalledOnce();
  expect(result.current.size).toBe(0);
});
