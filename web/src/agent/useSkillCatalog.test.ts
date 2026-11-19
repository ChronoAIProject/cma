import { ApiError, ProtocolError } from '@cma/client';
import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { SkillCatalogClient, SkillCatalogEntry } from '../profiles/client';
import { useSkillCatalog } from './useSkillCatalog';

type Page = Awaited<ReturnType<SkillCatalogClient['searchSkills']>>;
const entry = (id: string): SkillCatalogEntry => ({ id, name: id, versions: ['1.0'] });
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}
const tick = (ms = 200) => act(() => vi.advanceTimersByTimeAsync(ms));
beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

it('retains the query and failed page, deduplicates retry, and rejects cyclic cursors', async () => {
  const searchSkills = vi
    .fn<SkillCatalogClient['searchSkills']>()
    .mockResolvedValueOnce({ items: [] })
    .mockResolvedValueOnce({ items: [entry('one')], next: 'page-two' })
    .mockRejectedValueOnce(new ApiError(503, 'private'))
    .mockResolvedValueOnce({ items: [entry('one'), entry('two')], next: 'page-three' })
    .mockResolvedValueOnce({ items: [entry('bad')], next: 'page-two' });
  const source = { searchSkills };
  const { result } = renderHook(() => useSkillCatalog(source, true));
  expect(result.current.searchEnabled).toBe(false);
  await tick();
  expect(result.current.searchEnabled).toBe(true);
  act(() => result.current.search('typed query'));
  await tick();
  act(() => result.current.more());
  await tick();
  expect(result.current).toMatchObject({
    phase: 'failed',
    query: 'typed query',
    cursor: 'page-two',
    entries: [entry('one')],
    selectable: false,
  });
  act(() => result.current.retry());
  await tick();
  expect(searchSkills.mock.calls[2]?.[0]).toBe('typed query');
  expect(searchSkills.mock.calls[3]?.[1]?.cursor).toBe('page-two');
  expect(result.current.entries.map(({ id }) => id)).toEqual(['one', 'two']);
  act(() => result.current.more());
  await tick();
  expect(result.current.failure).toBeInstanceOf(ProtocolError);
  expect(result.current.entries.map(({ id }) => id)).toEqual(['one', 'two']);
});

it('aborts old queries, inactivity and sources and ignores late successes and failures', async () => {
  const oldQuery = deferred<Page>();
  const inactive = deferred<Page>();
  const oldSource = deferred<Page>();
  const searchSkills = vi
    .fn<SkillCatalogClient['searchSkills']>()
    .mockResolvedValueOnce({ items: [] })
    .mockReturnValueOnce(oldQuery.promise)
    .mockResolvedValueOnce({ items: [entry('new-query')] })
    .mockReturnValueOnce(inactive.promise)
    .mockReturnValueOnce(oldSource.promise);
  const initialSource = { searchSkills };
  const { result, rerender, unmount } = renderHook(
    ({ source, active }) => useSkillCatalog(source, active),
    { initialProps: { source: initialSource, active: true } },
  );
  await tick();
  act(() => result.current.search('old'));
  await tick();
  act(() => result.current.search('new'));
  expect(searchSkills.mock.calls[1]?.[1]?.signal?.aborted).toBe(true);
  await tick();
  await act(async () => oldQuery.resolve({ items: [entry('stale')] }));
  expect(result.current.entries).toEqual([entry('new-query')]);
  act(() => result.current.search('inactive'));
  await tick();
  rerender({ source: initialSource, active: false });
  expect(searchSkills.mock.calls[3]?.[1]?.signal?.aborted).toBe(true);
  await act(async () => inactive.reject(new ApiError(403, 'stale')));
  expect(result.current.failure).toBeUndefined();
  rerender({ source: initialSource, active: true });
  await tick();
  const nextSource = { searchSkills: vi.fn().mockResolvedValue({ items: [entry('next-source')] }) };
  rerender({ source: nextSource, active: true });
  expect(searchSkills.mock.calls[4]?.[1]?.signal?.aborted).toBe(true);
  await act(async () => oldSource.resolve({ items: [entry('stale-source')] }));
  await tick();
  expect(result.current.entries).toEqual([entry('next-source')]);
  unmount();
  expect(nextSource.searchSkills.mock.calls[0]?.[1].signal.aborted).toBe(true);
});

it('honors Retry-After across retries, searches and panel inactivity without retrying automatically', async () => {
  vi.spyOn(Math, 'random').mockReturnValue(0);
  const source = {
    searchSkills: vi
      .fn<SkillCatalogClient['searchSkills']>()
      .mockResolvedValueOnce({ items: [entry('one')], next: 'two' })
      .mockRejectedValueOnce(new ApiError(429, 'private', 'rate_limited', undefined, 3000))
      .mockResolvedValue({ items: [] }),
  };
  const { result, rerender } = renderHook(({ active }) => useSkillCatalog(source, active), {
    initialProps: { active: true },
  });
  await tick();
  act(() => result.current.more());
  await tick();
  expect(result.current.waiting).toBe(true);
  act(() => {
    result.current.retry();
    result.current.search('bypass');
  });
  await tick(2999);
  expect(source.searchSkills).toHaveBeenCalledTimes(2);
  expect(result.current.query).toBe('');
  await tick(1);
  expect(result.current.waiting).toBe(false);
  expect(source.searchSkills).toHaveBeenCalledTimes(2);
  act(() => result.current.retry());
  await tick();
  expect(source.searchSkills.mock.calls[2]?.[1]?.cursor).toBe('two');
  source.searchSkills.mockRejectedValueOnce(
    new ApiError(429, 'private', undefined, undefined, 3000),
  );
  act(() => result.current.search('next'));
  await tick();
  rerender({ active: false });
  rerender({ active: true });
  await tick(2999);
  expect(source.searchSkills).toHaveBeenCalledTimes(4);
  await tick(1);
  expect(source.searchSkills).toHaveBeenCalledTimes(5);
});

it.each([401, 403])('does not retry or search around a %s refusal', async (status) => {
  const source = { searchSkills: vi.fn().mockRejectedValue(new ApiError(status, 'private')) };
  const { result } = renderHook(() => useSkillCatalog(source, true));
  await tick();
  act(() => {
    result.current.retry();
    result.current.search('bypass');
  });
  await tick(5000);
  expect(result.current.phase).toBe('failed');
  expect(result.current.searchEnabled).toBe(false);
  expect(source.searchSkills).toHaveBeenCalledTimes(1);
});
