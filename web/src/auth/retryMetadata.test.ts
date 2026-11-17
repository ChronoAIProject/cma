import { expect, it, vi } from 'vitest';
import { ApiError, TransportError } from '@cma/client';
import type { AuthProvider, AuthState } from './types';
import { authHttpRetry, authTransportRetry, currentTransportUser } from './retryMetadata';

it.each([429, 502, 503, 504])(
  'retains only explicit temporary HTTP %s disposition and delay',
  (status) => {
    expect(authHttpRetry(status, { retryable: true }, 90_000)).toEqual({
      kind: 'http',
      status,
      retryAfterMs: 90_000,
    });
  },
);

it.each([200, 401, 403, 409, 422, 500])(
  'rejects contradictory retry permission on HTTP %s',
  (status) => {
    expect(authHttpRetry(status, { retryable: true }, 1000)).toBeUndefined();
  },
);

it.each([
  undefined,
  null,
  [],
  true,
  'true',
  {},
  { retryable: false },
  { retryable: 'true' },
  { code: 'identity_unavailable' },
])('does not infer retry permission from %j', (body) => {
  expect(authHttpRetry(503, body, 1000)).toBeUndefined();
});

it('distinguishes network loss and timeout from cancellation or arbitrary exceptions', () => {
  expect(authTransportRetry(new TypeError('network'))).toEqual({ kind: 'transport' });
  expect(authTransportRetry(new Error('timeout'), undefined, true)).toEqual({ kind: 'transport' });
  expect(authTransportRetry(new Error('identity unavailable'))).toBeUndefined();
  expect(authTransportRetry(new SyntaxError('malformed'))).toBeUndefined();
  const controller = new AbortController();
  controller.abort();
  expect(authTransportRetry(new TypeError('network'), controller.signal, true)).toBeUndefined();
});

it('adapts typed disposition without copying upstream payloads or inventing identity', async () => {
  const current = vi.fn<AuthProvider['current']>();
  const read = currentTransportUser({ current });
  current.mockResolvedValueOnce({
    status: 'error',
    message: 'Temporary',
    retry: { kind: 'http', status: 503, retryAfterMs: 3000 },
  });
  await expect(read()).rejects.toMatchObject({
    name: 'ApiError',
    status: 503,
    retryAfterMs: 3000,
    extensions: {},
  });
  current.mockResolvedValueOnce({
    status: 'error',
    message: 'Temporary',
    retry: { kind: 'transport' },
  });
  await expect(read()).rejects.toBeInstanceOf(TransportError);
  current.mockResolvedValueOnce({
    status: 'error',
    message: 'Temporary',
    reason: 'identity_unavailable',
  });
  const untyped = await read().catch((error: unknown) => error);
  expect(untyped).not.toBeInstanceOf(ApiError);
  expect(untyped).not.toBeInstanceOf(TransportError);
  current.mockResolvedValueOnce({ status: 'anonymous' });
  await expect(read()).resolves.toBeUndefined();
  current.mockResolvedValueOnce({
    status: 'authenticated',
    user: { id: 'owner', handle: 'Owner', capabilities: [], visitor: false },
  });
  await expect(read()).resolves.toBe('owner');
});

it('never exposes authority or retries after an abort-resistant identity read finishes', async () => {
  const controller = new AbortController();
  let release!: (state: AuthState) => void;
  const current = vi.fn<AuthProvider['current']>(
    () =>
      new Promise((resolve) => {
        release = resolve;
      }),
  );
  const read = currentTransportUser({ current });
  const pending = expect(read(controller.signal)).rejects.toMatchObject({ name: 'AbortError' });
  controller.abort();
  release({ status: 'error', message: 'Temporary', retry: { kind: 'transport' } });
  await pending;
  await expect(read(controller.signal)).rejects.toMatchObject({ name: 'AbortError' });
  expect(current).toHaveBeenCalledOnce();
});
