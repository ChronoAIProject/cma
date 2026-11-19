import { ApiError, ProtocolError, TimeoutError } from '@cma/client';
import { expect, it, vi } from 'vitest';
import { settingsFailure } from './settingsFailure';

it.each([400, 422])('classifies editable rejection %i without upstream content', (status) => {
  expect(settingsFailure(new ApiError(status, '<html>internal data</html>', 'unknown'))).toEqual({
    kind: 'invalid',
    message: 'The update was rejected. Check your changes, then try again.',
    retryAfterMs: 0,
  });
});
it.each([401, 403, 404, 410])('requires access recovery for %i', (status) => {
  expect(settingsFailure(new ApiError(status, 'private'))).toMatchObject({ kind: 'denied' });
});
it.each([undefined, 'agent_operation_in_flight', 'unexpected_code'])(
  'does not guess a conflict cause from %s',
  (code) => {
    expect(settingsFailure(new ApiError(409, 'The active turn is the cause.', code))).toEqual({
      kind: 'conflict',
      retryAfterMs: 0,
      message:
        'The Agent state changed or an update is in progress. Refresh the current settings, then apply your changes again.',
    });
  },
);
it.each(['agent_delivery_ambiguous', 'idempotency_in_flight', 'idempotency_recovery_required'])(
  'retains unresolved command code %s even when its status is 409',
  (code) => {
    expect(settingsFailure(new ApiError(409, 'private', code))).toMatchObject({ kind: 'retry' });
  },
);
it.each([
  new TypeError('network failure'),
  new TimeoutError(50),
  new ProtocolError('private'),
  new ApiError(503, 'private'),
])('keeps an unknown write outcome unconfirmed', (error) => {
  expect(settingsFailure(error)).toEqual({
    kind: 'retry',
    retryAfterMs: 0,
    message: 'The update outcome is unconfirmed. Retry the same request to check it.',
  });
});
it('honors a server deadline and falls back to the existing read backoff for invalid rate limits', () => {
  const random = vi.spyOn(Math, 'random').mockReturnValue(0);
  try {
    expect(
      settingsFailure(new ApiError(503, 'private', undefined, undefined, 45000)).retryAfterMs,
    ).toBe(45000);
    for (const retryAfterMs of [undefined, -1, Infinity, 2 ** 32])
      expect(
        settingsFailure(new ApiError(429, 'private', undefined, undefined, retryAfterMs))
          .retryAfterMs,
      ).toBe(1000);
  } finally {
    random.mockRestore();
  }
});
it('distinguishes an unavailable read without repeating error text', () => {
  expect(settingsFailure(new Error('<html>private</html>'), true).message).toBe(
    'Current settings could not be read. Your changes are kept. Refresh to try again.',
  );
});
