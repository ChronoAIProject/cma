import { expect, it } from 'vitest';
import { ApiError, CredentialError, ProtocolError, TransportError } from '../api-client/errors';
import { catalogFailure } from './catalogFailure';

it.each([
  [new ApiError(401, 'private'), 'sign-in', false],
  [new CredentialError('private'), 'sign-in', false],
  [new ApiError(403, 'private'), 'forbidden', false],
  [new ApiError(429, 'private'), 'rate-limit', true],
  [new ApiError(503, 'private'), 'unavailable', true],
  [new ProtocolError('private'), 'invalid', true],
  [new TransportError('private'), 'network', true],
  [new Error('private'), 'unknown', true],
] as const)('classifies %s without copying its diagnostics', (error, kind, retryable) => {
  const result = catalogFailure(error);
  expect(result).toMatchObject({ kind, retryable });
  expect(JSON.stringify(result)).not.toContain('private');
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
