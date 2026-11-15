import { afterEach, expect, it, vi } from 'vitest';
import { agentFilesClient } from './agentFiles';
import { PREVIEW_DEADLINE_MS } from './previewDeadline';

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});
const receipt = { path: 'a.txt', size_bytes: 2, sha256: 'a'.repeat(64), content_base64: 'aGk=' };

it('bounds stalled headers, aborts I/O and allows an explicit fresh retry', async () => {
  vi.useFakeTimers();
  let signal: AbortSignal | undefined;
  const fetcher = vi.fn((_url: string, init: RequestInit) => {
    signal = init.signal as AbortSignal;
    return new Promise<Response>(() => {});
  });
  vi.stubGlobal('fetch', fetcher);
  const read = agentFilesClient().read('agent', 'a.txt');
  const failed = expect(read).rejects.toMatchObject({ status: 408, code: 'file_preview_timeout' });
  await vi.advanceTimersByTimeAsync(PREVIEW_DEADLINE_MS);
  await failed;
  expect(signal?.aborted).toBe(true);
  expect(fetcher).toHaveBeenCalledTimes(1);
  fetcher.mockResolvedValueOnce(new Response(JSON.stringify(receipt)));
  await expect(agentFilesClient().read('agent', 'a.txt')).resolves.toEqual(receipt);
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(vi.getTimerCount()).toBe(0);
});

it('includes stalled body consumption in the deadline', async () => {
  vi.useFakeTimers();
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response(new ReadableStream({ start() {} }))),
  );
  const failed = expect(agentFilesClient().read('agent', 'a.txt')).rejects.toMatchObject({
    code: 'file_preview_timeout',
  });
  await vi.advanceTimersByTimeAsync(PREVIEW_DEADLINE_MS);
  await failed;
});

it('preserves access refusals and does not retry', async () => {
  vi.useFakeTimers();
  const fetcher = vi.fn(
    async () => new Response(JSON.stringify({ code: 'resource_not_found' }), { status: 404 }),
  );
  vi.stubGlobal('fetch', fetcher);
  await expect(agentFilesClient().read('agent', 'a.txt')).rejects.toMatchObject({
    status: 404,
    code: 'resource_not_found',
  });
  await vi.advanceTimersByTimeAsync(PREVIEW_DEADLINE_MS * 2);
  expect(fetcher).toHaveBeenCalledTimes(1);
  expect(vi.getTimerCount()).toBe(0);
});

it('cancels obsolete reads without publishing a late response', async () => {
  vi.useFakeTimers();
  let finish!: (response: Response) => void;
  let requestSignal: AbortSignal | undefined;
  const fetcher = vi.fn((_url: string, init: RequestInit) => {
    requestSignal = init.signal as AbortSignal;
    return new Promise<Response>((resolve) => {
      finish = resolve;
    });
  });
  vi.stubGlobal('fetch', fetcher);
  const owner = new AbortController();
  const failed = expect(
    agentFilesClient().read('agent', 'a.txt', { signal: owner.signal }),
  ).rejects.toMatchObject({ name: 'AbortError' });
  owner.abort();
  await failed;
  expect(requestSignal?.aborted).toBe(true);
  finish(new Response(JSON.stringify(receipt)));
  await vi.advanceTimersByTimeAsync(0);
  expect(vi.getTimerCount()).toBe(0);
  await expect(
    agentFilesClient().read('agent', 'a.txt', { signal: owner.signal }),
  ).rejects.toMatchObject({ name: 'AbortError' });
  expect(fetcher).toHaveBeenCalledTimes(1);
});
