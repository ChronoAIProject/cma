import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { BusinessReadError, readBusinessProblem } from './businessReadProblem';
import { retryLifecycleRead } from '../readRetry';

beforeEach(() =>
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'Date', 'performance'] }),
);
afterEach(() => vi.useRealTimers());
const signal = () => new AbortController().signal;
function response(body: string, status = 503, retry = '3') {
  return new Response(body, {
    status,
    headers: { 'Retry-After': retry, 'X-Request-Id': 'synthetic-request' },
  });
}

it.each([
  ['', 'legacy-empty', undefined, true],
  ['   ', 'legacy-empty', undefined, true],
  ['{"code":"resource_not_found"}', 'problem', undefined, true],
  ['{"detail":"Unavailable"}', 'problem', undefined, true],
  ['{"status":503,"code":"store_failed","retryable":false}', 'problem', false, false],
  ['{"code":"busy","retryable":true}', 'problem', true, true],
  ['{"code":"busy"}', 'problem', undefined, true],
  ['{"code":"busy","retryable":"false"}', 'invalid', undefined, false],
  ['{"code":"busy","retryable":0}', 'invalid', undefined, false],
  ['{"code":"busy","retryable":null}', 'invalid', undefined, false],
  ['{"code":"busy","status":200,"retryable":false}', 'invalid', false, false],
  ['{"code":"busy","status":"503","retryable":true}', 'invalid', true, false],
  ['{"code":12,"retryable":true}', 'invalid', true, false],
  ['{"code":"busy","detail":null,"retryable":true}', 'invalid', true, false],
  ['{}', 'invalid', undefined, false],
  ['null', 'invalid', undefined, false],
  ['[]', 'invalid', undefined, false],
  ['"unavailable"', 'invalid', undefined, false],
  ['<html>upstream unavailable</html>', 'invalid', undefined, false],
  ['{', 'invalid', undefined, false],
] as const)(
  'decodes %s without granting unsupported retry eligibility',
  async (body, shape, retryable, eligible) => {
    const problem = await readBusinessProblem(response(body));
    expect(problem).toMatchObject({
      status: 503,
      shape,
      retryAfterMs: 3000,
      requestId: 'synthetic-request',
    });
    expect(problem.retryable).toBe(retryable);
    expect(Object.isFrozen(problem)).toBe(true);
    const error = new BusinessReadError(problem);
    expect(error.extensions.retryable).toBe(retryable);
    expect(Object.isFrozen(error.extensions)).toBe(true);
    expect(retryLifecycleRead(error, signal())).toBe(eligible);
    expect(vi.getTimerCount()).toBe(0);
  },
);

it.each([401, 403, 404])('retains fatal HTTP %i despite a true extension', async (status) => {
  const problem = await readBusinessProblem(
    response('{"code":"refused","retryable":true}', status),
  );
  expect(problem.status).toBe(status);
  expect(retryLifecycleRead(new BusinessReadError(problem), signal())).toBe(false);
});

it.each(['0', '-1', '1.5', '99999999999999999999', 'not-a-date', ''])(
  'rejects malformed retry header %s',
  async (header) => {
    expect((await readBusinessProblem(response('', 503, header))).retryAfterMs).toBeUndefined();
  },
);

it('accepts an HTTP date once and ignores subsequent wall clock jumps', async () => {
  vi.setSystemTime(new Date('2026-11-23T00:00:00Z'));
  const at = performance.now();
  const problem = await readBusinessProblem(response('', 503, 'Mon, 23 Nov 2026 00:00:03 GMT'));
  expect(problem.retryAfterMs).toBe(3000);
  vi.setSystemTime(new Date('2030-01-01'));
  expect(problem.retryAt).toBe(at + 3000);
});

it('does not retain an unusable response request identifier', async () => {
  const value = response('');
  const get = value.headers.get.bind(value.headers);
  vi.spyOn(value.headers, 'get').mockImplementation((key) =>
    key === 'x-request-id' ? 'bad\tidentifier' : get(key),
  );
  expect((await readBusinessProblem(value)).requestId).toBeUndefined();
});

it('never exposes upstream details or unknown extensions in the error', async () => {
  const problem = await readBusinessProblem(
    response(
      JSON.stringify({
        detail: 'private://upstream/owner',
        code: 'store_failed',
        retryable: false,
        credential: 'synthetic-private-value',
        owner: 'synthetic-private-owner',
      }),
    ),
  );
  const error = new BusinessReadError(problem);
  expect(error.message).toBe('Agent lifecycle status failed (503).');
  expect(JSON.stringify(error)).not.toMatch(/private|credential|owner/);
});

it.each([false, true])(
  'bounds oversized metadata and cancels the body (declared=%s)',
  async (declared) => {
    const cancel = vi.fn();
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        controller.enqueue(new Uint8Array(64 * 1024 + 1));
      },
      cancel,
    });
    const value = new Response(stream, {
      status: 503,
      headers: {
        'Retry-After': '3',
        ...(declared ? { 'Content-Length': String(64 * 1024 + 1) } : {}),
      },
    });
    const problem = await readBusinessProblem(value);
    expect(problem).toMatchObject({ status: 503, shape: 'invalid', retryAfterMs: 3000 });
    expect(cancel).toHaveBeenCalledOnce();
    expect(retryLifecycleRead(new BusinessReadError(problem), signal())).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
  },
);

it('accepts a valid document exactly at the 64 KiB bound', async () => {
  const prefix = '{"code":"busy","detail":"';
  const suffix = '"}';
  const body = prefix + 'a'.repeat(64 * 1024 - prefix.length - suffix.length) + suffix;
  expect((await readBusinessProblem(response(body))).shape).toBe('problem');
});

it('marks stalled metadata invalid after one second and cancels its reader', async () => {
  const cancel = vi.fn();
  const value = new Response(new ReadableStream({ cancel }), {
    status: 503,
    headers: { 'Retry-After': '3' },
  });
  const result = readBusinessProblem(value);
  await vi.advanceTimersByTimeAsync(999);
  expect(cancel).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect(await result).toMatchObject({
    status: 503,
    shape: 'invalid',
    retryAfterMs: 3000,
    retryAt: 3000,
  });
  expect(cancel).toHaveBeenCalledOnce();
  expect(vi.getTimerCount()).toBe(0);
});

it('counts decoding time toward the received floor', async () => {
  let stream!: ReadableStreamDefaultController<Uint8Array>;
  const result = readBusinessProblem(
    new Response(
      new ReadableStream({
        start(value) {
          stream = value;
        },
      }),
      {
        status: 503,
        headers: { 'Retry-After': '3' },
      },
    ),
  );
  await vi.advanceTimersByTimeAsync(900);
  stream.enqueue(new TextEncoder().encode('{"code":"busy"}'));
  stream.close();
  expect(await result).toMatchObject({ shape: 'problem', retryAt: 3000, retryAfterMs: 3000 });
  expect(vi.getTimerCount()).toBe(0);
});

it.each([true, false])(
  'preserves caller cancellation before/during body (before=%s)',
  async (before) => {
    const caller = new AbortController();
    const cancel = vi.fn();
    if (before) caller.abort();
    const result = readBusinessProblem(
      new Response(new ReadableStream({ cancel }), { status: 503 }),
      caller.signal,
    );
    const cancelled = expect(result).rejects.toMatchObject({ name: 'AbortError' });
    if (!before) caller.abort();
    await cancelled;
    await vi.advanceTimersByTimeAsync(0);
    expect(cancel).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
  },
);
