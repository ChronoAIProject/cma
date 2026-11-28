import { afterEach, expect, it, vi } from 'vitest';
import { agentFilesClient } from './agentFiles';

const content = { path: 'a.txt', size_bytes: 2, sha256: 'a'.repeat(64), content_base64: 'aGk=' };
const refused = () =>
  new Response(
    JSON.stringify({
      status: 503,
      code: 'store_failed',
      retryable: false,
      detail: 'private upstream detail',
    }),
    { status: 503, headers: { 'Retry-After': '3', 'X-Request-Id': 'synthetic-read' } },
  );

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

it('preserves file refusal metadata with safe presentation', async () => {
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => refused()),
  );
  const error = await agentFilesClient()
    .read('agent', 'a.txt')
    .catch((error: unknown) => error);
  expect(error).toMatchObject({
    status: 503,
    code: 'store_failed',
    retryable: false,
    retryAfterMs: 3000,
    requestId: 'synthetic-read',
  });
  expect((error as Error).message).not.toContain('private upstream');
});

it('waits for the file floor only when explicitly retried and dispatches once', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date', 'performance'] });
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(refused())
    .mockResolvedValueOnce(new Response(JSON.stringify(content)));
  vi.stubGlobal('fetch', fetcher);
  const client = agentFilesClient();
  await expect(client.read('agent', 'a.txt')).rejects.toMatchObject({ status: 503 });
  await vi.advanceTimersByTimeAsync(1000);
  expect(fetcher).toHaveBeenCalledOnce();
  const retry = client.read('agent', 'a.txt');
  await vi.advanceTimersByTimeAsync(1999);
  expect(fetcher).toHaveBeenCalledOnce();
  await vi.advanceTimersByTimeAsync(1);
  await expect(retry).resolves.toEqual(content);
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(vi.getTimerCount()).toBe(0);
});

it('keeps long floors across the unchanged four-second preview deadline', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date', 'performance'] });
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(
      new Response('{"code":"busy"}', {
        status: 429,
        headers: { 'Retry-After': '10' },
      }),
    )
    .mockResolvedValueOnce(new Response(JSON.stringify(content)));
  vi.stubGlobal('fetch', fetcher);
  const client = agentFilesClient();
  await expect(client.read('agent', 'a.txt')).rejects.toMatchObject({ status: 429 });
  for (let attempt = 0; attempt < 2; attempt++) {
    const failed = expect(client.read('agent', 'a.txt')).rejects.toMatchObject({
      status: 408,
      code: 'file_preview_timeout',
    });
    await vi.advanceTimersByTimeAsync(3999);
    expect(fetcher).toHaveBeenCalledOnce();
    await vi.advanceTimersByTimeAsync(1);
    await failed;
    expect(vi.getTimerCount()).toBe(0);
  }
  const later = client.read('agent', 'a.txt');
  await vi.advanceTimersByTimeAsync(1999);
  expect(fetcher).toHaveBeenCalledOnce();
  await vi.advanceTimersByTimeAsync(1);
  await expect(later).resolves.toEqual(content);
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(vi.getTimerCount()).toBe(0);
});

it('shares a floor across file selections, but isolates Agents and clients', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date', 'performance'] });
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(refused())
    .mockImplementation(async (url: string) => {
      const path = new URL(url, 'https://cma.test').searchParams.get('path');
      return new Response(JSON.stringify({ ...content, path }));
    });
  vi.stubGlobal('fetch', fetcher);
  const client = agentFilesClient();
  await expect(client.read('agent', 'a.txt')).rejects.toMatchObject({ status: 503 });
  const owner = new AbortController();
  const first = expect(
    client.read('agent', 'b.txt', { signal: owner.signal }),
  ).rejects.toMatchObject({ name: 'AbortError' });
  await vi.advanceTimersByTimeAsync(500);
  owner.abort();
  await first;
  await client.read('other-agent', 'a.txt');
  await agentFilesClient().read('agent', 'a.txt');
  expect(fetcher).toHaveBeenCalledTimes(3);
  const next = client.read('agent', 'b.txt');
  await vi.advanceTimersByTimeAsync(2499);
  expect(fetcher).toHaveBeenCalledTimes(3);
  await vi.advanceTimersByTimeAsync(1);
  await expect(next).resolves.toMatchObject({ path: 'b.txt' });
  expect(fetcher).toHaveBeenCalledTimes(4);
  expect(vi.getTimerCount()).toBe(0);
});

it('cancels a refused preview body and retains the already received floor', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date', 'performance'] });
  const cancel = vi.fn();
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(
      new Response(new ReadableStream({ cancel }), {
        status: 503,
        headers: { 'Retry-After': '3' },
      }),
    )
    .mockResolvedValueOnce(new Response(JSON.stringify(content)));
  vi.stubGlobal('fetch', fetcher);
  const client = agentFilesClient();
  const owner = new AbortController();
  const failed = expect(
    client.read('agent', 'a.txt', { signal: owner.signal }),
  ).rejects.toMatchObject({ name: 'AbortError' });
  await vi.advanceTimersByTimeAsync(500);
  owner.abort();
  await failed;
  await vi.advanceTimersByTimeAsync(0);
  expect(cancel).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
  const next = client.read('agent', 'a.txt');
  await vi.advanceTimersByTimeAsync(2499);
  expect(fetcher).toHaveBeenCalledOnce();
  await vi.advanceTimersByTimeAsync(1);
  await next;
  expect(fetcher).toHaveBeenCalledTimes(2);
});

it('does not apply the preview floor or retry scheduling to list/download/mutations', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date', 'performance'] });
  const fetcher = vi.fn(async () => refused());
  vi.stubGlobal('fetch', fetcher);
  const client = agentFilesClient();
  for (const attempt of [
    () => client.read('agent', 'a.txt'),
    () => client.list('agent', ''),
    () => client.download('agent', 'a.txt'),
    () => client.write('agent', { path: 'a.txt', content_base64: 'aGk=' }),
    () => client.remove('agent', 'a.txt', false),
  ])
    await expect(attempt()).rejects.toMatchObject({ status: 503 });
  expect(fetcher).toHaveBeenCalledTimes(5);
  await vi.advanceTimersByTimeAsync(60_000);
  expect(fetcher).toHaveBeenCalledTimes(5);
  expect(vi.getTimerCount()).toBe(0);
});

it.each(['null', '[]', '"unexpected"', '{'])(
  'rejects malformed successful preview %s as a protocol failure',
  async (body) => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => new Response(body)),
    );
    await expect(agentFilesClient().read('agent', 'a.txt')).rejects.toMatchObject({
      name: 'ProtocolError',
    });
  },
);

it.each([
  [401, 'not_authenticated', 'Sign in again to download the file.'],
  [404, 'resource_not_found', 'File access is no longer available.'],
  [503, 'file_unavailable', 'Download failed. Try downloading the file again.'],
])('decodes a file download refusal with HTTP %i', async (status, code, message) => {
  vi.stubGlobal('fetch', async () => Response.json({ code }, { status }));
  await expect(agentFilesClient().download('agent', 'a.txt')).rejects.toMatchObject({
    status,
    code,
    message: expect.stringContaining(message),
  });
});

it('rejects HTML at the file download endpoint without accepting redirects', async () => {
  const headers = { 'content-type': 'text/html' };
  const fetcher = vi.fn(async () => new Response('<html>Sign in</html>', { headers }));
  vi.stubGlobal('fetch', fetcher);
  await expect(agentFilesClient().download('agent', 'a.txt')).rejects.toMatchObject({
    code: 'invalid_file_download',
  });
  expect(fetcher).toHaveBeenCalledWith(
    '/api/v1/agents/agent/files/download?path=a.txt',
    expect.objectContaining({ cache: 'no-store', credentials: 'same-origin', redirect: 'error' }),
  );
});

it('refuses a download whose declared size exceeds 32 MiB', async () => {
  const fetcher = vi.fn(
    async () =>
      new Response('fixture', {
        headers: {
          'content-type': 'application/octet-stream',
          'content-length': String(32 * 1024 * 1024 + 1),
        },
      }),
  );
  vi.stubGlobal('fetch', fetcher);

  await expect(agentFilesClient().download('agent', 'a.txt')).rejects.toMatchObject({
    code: 'file_download_failed',
  });
});

it('includes late headers and metadata cancellation in the four-second preview budget', async () => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date', 'performance'] });
  const cancel = vi.fn();
  const fetcher = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise<Response>((resolve) => {
          setTimeout(
            () =>
              resolve(
                new Response(new ReadableStream({ cancel }), {
                  status: 503,
                  headers: { 'Retry-After': '3' },
                }),
              ),
            3500,
          );
        }),
    )
    .mockResolvedValueOnce(new Response(JSON.stringify(content)));
  vi.stubGlobal('fetch', fetcher);
  const client = agentFilesClient();
  const failed = expect(client.read('agent', 'a.txt')).rejects.toMatchObject({
    code: 'file_preview_timeout',
  });
  await vi.advanceTimersByTimeAsync(3999);
  expect(cancel).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  await failed;
  expect(cancel).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
  const retry = client.read('agent', 'a.txt');
  await vi.advanceTimersByTimeAsync(2499);
  expect(fetcher).toHaveBeenCalledOnce();
  await vi.advanceTimersByTimeAsync(1);
  await retry;
  expect(fetcher).toHaveBeenCalledTimes(2);
  expect(vi.getTimerCount()).toBe(0);
});
