import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import {
  ApiError,
  CredentialError,
  ProtocolError,
  RequestAborted,
  TimeoutError,
  TransportError,
} from '@cma/client';
import { readGate, retryLifecycleRead } from './readRetry';
import { BusinessReadError, readBusinessProblem } from './rest/businessReadProblem';

beforeEach(() =>
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date', 'performance'] }),
);
afterEach(() => vi.useRealTimers());
const signal = () => new AbortController().signal;
async function refusal(seconds = 3) {
  return new BusinessReadError(
    await readBusinessProblem(
      new Response('{"code":"busy","retryable":true}', {
        status: 503,
        headers: { 'Retry-After': String(seconds) },
      }),
    ),
  );
}
async function remember(client: object, key: string, seconds = 3) {
  const error = await refusal(seconds);
  await expect(
    readGate(client, key).run(signal(), async () => {
      throw error;
    }),
  ).rejects.toBe(error);
}

it.each([408, 429, 500, 502, 503, 504])(
  'keeps legacy HTTP %i recovery, with false overriding it',
  (status) => {
    expect(retryLifecycleRead(new ApiError(status, ''), signal())).toBe(true);
    expect(
      retryLifecycleRead(
        new ApiError(status, '', undefined, undefined, undefined, { retryable: false }),
        signal(),
      ),
    ).toBe(false);
    expect(
      retryLifecycleRead(
        new ApiError(status, '', undefined, undefined, undefined, { retryable: true }),
        signal(),
      ),
    ).toBe(true);
  },
);
it.each([400, 401, 403, 404, 409, 422])('never promotes HTTP %i into automatic retry', (status) => {
  expect(
    retryLifecycleRead(
      new ApiError(status, '', undefined, undefined, undefined, { retryable: true }),
      signal(),
    ),
  ).toBe(false);
});
it.each(['false', 0, null])(
  'rejects malformed retryable %s from an injected client',
  (retryable) => {
    expect(
      retryLifecycleRead(
        new ApiError(503, '', undefined, undefined, undefined, { retryable }),
        signal(),
      ),
    ).toBe(false);
  },
);
it.each([
  new ProtocolError('bad identity'),
  new CredentialError('unavailable'),
  new Error('bug'),
  new ReferenceError('bug'),
  new TypeError('programming bug'),
  new RequestAborted(),
  null,
])('does not retry arbitrary or terminal failures: %s', (error) => {
  expect(retryLifecycleRead(error, signal())).toBe(false);
});
it.each([new TransportError('network'), new TimeoutError(1000)])(
  'keeps abort-aware network recovery: %s',
  (error) => {
    expect(retryLifecycleRead(error, signal())).toBe(true);
    const owner = new AbortController();
    owner.abort();
    expect(retryLifecycleRead(error, owner.signal)).toBe(false);
  },
);

it('keeps a live floor through cancelled waits, recreation and wall-clock jumps', async () => {
  const client = {};
  await remember(client, 'resource');
  const first = new AbortController();
  const read = vi.fn(async () => 'fresh');
  const cancelled = expect(
    readGate(client, 'resource').run(first.signal, read),
  ).rejects.toMatchObject({ name: 'AbortError' });
  await vi.advanceTimersByTimeAsync(500);
  first.abort();
  await cancelled;
  expect(vi.getTimerCount()).toBe(0);
  vi.setSystemTime(new Date('2040-01-01'));
  const next = readGate(client, 'resource').run(signal(), read);
  await vi.advanceTimersByTimeAsync(1000);
  vi.setSystemTime(new Date('2000-01-01'));
  await vi.advanceTimersByTimeAsync(1499);
  expect(read).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect(await next).toBe('fresh');
  expect(read).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
});

it('isolates clients and resource keys without caching any successful value', async () => {
  const client = {};
  await remember(client, 'a');
  const read = vi.fn(async () => 'fresh');
  await expect(readGate({}, 'a').run(signal(), read)).resolves.toBe('fresh');
  await expect(readGate(client, 'b').run(signal(), read)).resolves.toBe('fresh');
  await expect(readGate(client, 'b').run(signal(), read)).resolves.toBe('fresh');
  expect(read).toHaveBeenCalledTimes(3);
  expect(vi.getTimerCount()).toBe(0);
});

it('rechecks a floor extended by another response when an old timer wakes', async () => {
  const client = {};
  let reject!: (error: unknown) => void;
  const inflight = readGate(client, 'a').run(
    signal(),
    () =>
      new Promise((_resolve, fail) => {
        reject = fail;
      }),
  );
  const rejected = expect(inflight).rejects.toBeInstanceOf(ApiError);
  await remember(client, 'a');
  const read = vi.fn(async () => 'fresh');
  const waiting = readGate(client, 'a').run(signal(), read);
  await vi.advanceTimersByTimeAsync(1000);
  reject(await refusal(5));
  await rejected;
  await vi.advanceTimersByTimeAsync(4999);
  expect(read).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  await waiting;
  expect(read).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
});

it('an older success cannot erase a new response floor', async () => {
  const client = {};
  let resolve!: (value: string) => void;
  const older = readGate(client, 'a').run(
    signal(),
    () =>
      new Promise<string>((done) => {
        resolve = done;
      }),
  );
  await remember(client, 'a');
  resolve('older');
  await older;
  const read = vi.fn(async () => 'fresh');
  const waiting = readGate(client, 'a').run(signal(), read);
  await vi.advanceTimersByTimeAsync(2999);
  expect(read).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  await waiting;
  expect(read).toHaveBeenCalledOnce();
});

it('uses the longer of concurrent refusals, including after an older error arrives', async () => {
  const client = {};
  let reject!: (error: unknown) => void;
  const shorter = await refusal(1);
  const pending = readGate(client, 'a').run(
    signal(),
    () =>
      new Promise((_resolve, fail) => {
        reject = fail;
      }),
  );
  const failed = expect(pending).rejects.toBe(shorter);
  await remember(client, 'a', 3);
  reject(shorter);
  await failed;
  const read = vi.fn(async () => 'fresh');
  const waiting = readGate(client, 'a').run(signal(), read);
  await vi.advanceTimersByTimeAsync(2999);
  expect(read).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  await waiting;
  expect(read).toHaveBeenCalledOnce();
});

it('bounds active keys without evicting live floors, then expires them', async () => {
  const client = {};
  for (let index = 0; index < 65; index++)
    await remember(client, String(index), index === 64 ? 6 : 3);
  const read = vi.fn(async () => 'fresh');
  const oldest = readGate(client, '0').run(signal(), read);
  const overflow = readGate(client, '64').run(signal(), read);
  const unknown = readGate(client, '65').run(signal(), read);
  await vi.advanceTimersByTimeAsync(5999);
  expect(read).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  await Promise.all([oldest, overflow, unknown]);
  expect(read).toHaveBeenCalledTimes(3);
  await expect(readGate(client, '66').run(signal(), read)).resolves.toBe('fresh');
  expect(vi.getTimerCount()).toBe(0);
});

it('does not restart a received delay when a client reuses the same error', async () => {
  const client = {};
  const error = new ApiError(503, '', undefined, undefined, 3000);
  const read = vi.fn(async () => {
    throw error;
  });
  await expect(readGate(client, 'a').run(signal(), read)).rejects.toBe(error);
  const next = readGate(client, 'a').run(signal(), read);
  const rejected = expect(next).rejects.toBe(error);
  await vi.advanceTimersByTimeAsync(3000);
  await rejected;
  await expect(readGate(client, 'a').run(signal(), read)).rejects.toBe(error);
  expect(read).toHaveBeenCalledTimes(3);
  expect(vi.getTimerCount()).toBe(0);
});

it('retains a header floor even when its metadata body is cancelled', async () => {
  const client = {};
  const owner = new AbortController();
  const cancel = vi.fn();
  const pending = readGate(client, 'a').run(owner.signal, async (signal) => {
    throw new BusinessReadError(
      await readBusinessProblem(
        new Response(new ReadableStream({ cancel }), {
          status: 503,
          headers: { 'Retry-After': '3' },
        }),
        signal,
      ),
    );
  });
  const rejected = expect(pending).rejects.toMatchObject({ name: 'AbortError' });
  await vi.advanceTimersByTimeAsync(500);
  owner.abort();
  await rejected;
  expect(cancel).toHaveBeenCalledOnce();
  const read = vi.fn(async () => 'fresh');
  const next = readGate(client, 'a').run(signal(), read);
  await vi.advanceTimersByTimeAsync(2499);
  expect(read).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  await next;
  expect(read).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
});

it('publishes the header floor before decoding so an immediate replacement cannot bypass it', async () => {
  const client = {};
  const owner = new AbortController();
  const first = readGate(client, 'a').run(owner.signal, async (signal) => {
    throw new BusinessReadError(
      await readBusinessProblem(
        new Response(new ReadableStream(), {
          status: 503,
          headers: { 'Retry-After': '3' },
        }),
        signal,
      ),
    );
  });
  const cancelled = expect(first).rejects.toMatchObject({ name: 'AbortError' });
  await vi.advanceTimersByTimeAsync(100);
  owner.abort();
  const read = vi.fn(async () => 'fresh');
  const replacement = readGate(client, 'a').run(signal(), read);
  await cancelled;
  await vi.advanceTimersByTimeAsync(2899);
  expect(read).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  await replacement;
  expect(read).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
});

it('isolates timing observers even when two clients share a caller signal', async () => {
  const firstClient = {};
  const secondClient = {};
  const owner = new AbortController();
  let finish!: (value: string) => void;
  const other = readGate(secondClient, 'a').run(
    owner.signal,
    () =>
      new Promise<string>((resolve) => {
        finish = resolve;
      }),
  );
  await expect(
    readGate(firstClient, 'a').run(owner.signal, async (signal) => {
      throw new BusinessReadError(
        await readBusinessProblem(
          new Response('', {
            status: 503,
            headers: { 'Retry-After': '3' },
          }),
          signal,
        ),
      );
    }),
  ).rejects.toMatchObject({ status: 503 });
  finish('first value');
  await other;
  const read = vi.fn(async () => 'fresh');
  await expect(readGate(secondClient, 'a').run(owner.signal, read)).resolves.toBe('fresh');
  expect(read).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
});
