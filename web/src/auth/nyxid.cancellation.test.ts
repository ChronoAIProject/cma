import { afterEach, expect, it, vi } from 'vitest';
import { createNyxidAuthProvider } from './nyxid';

afterEach(() => vi.unstubAllGlobals());

it('forwards cancellation to the session fetch without treating failure as signed out', async () => {
  const request = new AbortController();
  let fetchedSignal: AbortSignal | undefined;
  vi.stubGlobal(
    'fetch',
    vi.fn((_url: string, init: RequestInit) => {
      fetchedSignal = init.signal!;
      return new Promise((_resolve, reject) => {
        fetchedSignal!.addEventListener('abort', () => reject(fetchedSignal!.reason), {
          once: true,
        });
      });
    }),
  );
  const result = createNyxidAuthProvider().current(request.signal);
  expect(fetchedSignal?.aborted).toBe(false);
  request.abort();
  expect(fetchedSignal?.aborted).toBe(true);
  await expect(result).resolves.toMatchObject({ status: 'error', reason: 'service_unavailable' });
});
