import { afterEach, expect, it, vi } from 'vitest';
import { ApiError, TransportError } from '@cma/client';
import { createNyxidAuthProvider } from './nyxid';
import { currentTransportUser } from './retryMetadata';

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
});
const response = (status: number, body: unknown, after = '3') =>
  new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/problem+json', 'retry-after': after },
  });

it.each([429, 502, 503, 504])(
  'propagates an explicitly retryable HTTP %s auth response through the real provider',
  async (status) => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => response(status, { code: 'identity_unavailable', retryable: true })),
    );
    const read = currentTransportUser(createNyxidAuthProvider());
    await expect(read()).rejects.toMatchObject({ name: 'ApiError', status, retryAfterMs: 3000 });
  },
);

it.each([false, 'true', undefined])(
  'leaves false, malformed and older metadata fail-closed: %s',
  async (retryable) => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => response(503, { code: 'store_failed', retryable })),
    );
    const error = await currentTransportUser(createNyxidAuthProvider())().catch(
      (error: unknown) => error,
    );
    expect(error).toBeInstanceOf(Error);
    expect(error).not.toBeInstanceOf(ApiError);
    expect(error).not.toBeInstanceOf(TransportError);
  },
);

it.each([401, 403])(
  'never retries HTTP %s despite a contradictory response field',
  async (status) => {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => response(status, { authenticated: false, retryable: true })),
    );
    await expect(currentTransportUser(createNyxidAuthProvider())()).resolves.toBeUndefined();
  },
);

it('preserves valid long/date Retry-After floors and rejects invalid values using the SDK parser', async () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-11-14T00:00:00Z'));
  for (const [header, expected] of [
    ['90', 90_000],
    ['Sat, 14 Nov 2026 00:01:30 GMT', 90_000],
    ['not-a-date', undefined],
  ] as const) {
    vi.stubGlobal(
      'fetch',
      vi.fn(async () => response(503, { retryable: true }, header)),
    );
    await expect(currentTransportUser(createNyxidAuthProvider())()).rejects.toMatchObject({
      retryAfterMs: expected,
    });
  }
});

it('keeps network loss typed but malformed identity JSON untyped', async () => {
  vi.stubGlobal('fetch', vi.fn().mockRejectedValueOnce(new TypeError('network')));
  await expect(currentTransportUser(createNyxidAuthProvider())()).rejects.toBeInstanceOf(
    TransportError,
  );
  vi.stubGlobal(
    'fetch',
    vi.fn(async () => new Response('{', { status: 200 })),
  );
  const error = await currentTransportUser(createNyxidAuthProvider())().catch(
    (error: unknown) => error,
  );
  expect(error).not.toBeInstanceOf(TransportError);
  expect(error).not.toBeInstanceOf(ApiError);
});
