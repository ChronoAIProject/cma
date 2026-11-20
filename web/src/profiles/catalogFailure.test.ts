import {
  ApiError,
  CredentialError,
  ProtocolError,
  TransportError,
  ValidationError,
} from '@cma/client';
import { expect, it } from 'vitest';
import { catalogFailure } from './catalogFailure';

const injected = '<html>synthetic-private-body</html>';
it.each([
  [new ApiError(401, injected), 'sign_in', false],
  [new CredentialError(injected), 'sign_in', false],
  [new ApiError(403, injected), 'forbidden', false],
  [new ApiError(429, injected), 'rate_limited', true],
  [new ApiError(503, injected), 'unavailable', true],
  [new ProtocolError(injected), 'invalid_response', true],
  [new TransportError(injected), 'unavailable', true],
  [new ValidationError(injected), 'invalid_query', false],
  [new ApiError(400, injected, 'invalid_profile_authoring_query'), 'invalid_query', false],
  [new Error(injected), 'unavailable', true],
])('classifies failures without copying their messages %#', (cause, kind, retry) => {
  const failure = catalogFailure(cause);
  expect(failure).toMatchObject({ kind, retry });
  expect(JSON.stringify(failure)).not.toContain(injected);
});

it('only exposes bounded, allowlisted diagnostic fields', () => {
  expect(
    catalogFailure(
      new ApiError(503, injected, 'profile_skills_unavailable', 'request_123:abc-1.2', undefined, {
        detail: injected,
      }),
    ),
  ).toMatchObject({
    status: 503,
    code: 'profile_skills_unavailable',
    requestId: 'request_123:abc-1.2',
  });
  for (const requestId of [injected, ' leading', 'newline\n', 'a'.repeat(81), '']) {
    expect(catalogFailure(new ApiError(503, injected, injected, requestId))).toMatchObject({
      code: undefined,
      requestId: undefined,
    });
  }
  for (const status of [NaN, Infinity, 200, 600, 503.5]) {
    expect(catalogFailure(new ApiError(status, injected)).status).toBeUndefined();
  }
  expect(catalogFailure({ status: 503, message: injected, requestId: injected })).toEqual({
    kind: 'unavailable',
    message: 'The catalog is temporarily unavailable. Try again.',
    retry: true,
  });
});

it('allows only known codes and bounded correlation IDs, never raw details or extensions', () => {
  const error = new ApiError(
    503,
    '<html>private</html>',
    'profile_services_unavailable',
    'catalog-request:01',
    2000,
    { detail: 'private', token: 'private' },
  );
  expect(catalogFailure(error)).toMatchObject({
    status: 503,
    code: 'profile_services_unavailable',
    requestId: 'catalog-request:01',
    retryAfterMs: 2000,
  });
  for (const requestId of ['', '<html>private</html>', 'private\nheader', 'a'.repeat(65)]) {
    const result = catalogFailure(new ApiError(503, 'private', 'private', requestId, NaN));
    expect(result.requestId).toBeUndefined();
    expect(result.code).toBeUndefined();
    expect(result.retryAfterMs).toBeUndefined();
    expect(JSON.stringify(result)).not.toContain('private');
  }
});
