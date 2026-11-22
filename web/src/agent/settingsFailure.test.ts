import { ApiError, ProtocolError, TimeoutError } from '@cma/client';
import { expect, it, vi } from 'vitest';
import { settingsFailure, settingsWriteFailure } from './settingsFailure';

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
      kind: 'unknown_conflict',
      retryAfterMs: 0,
      message: 'The update outcome is unconfirmed. Refresh, then retry the same request.',
    });
  },
);
it.each([
  ['dispatch_in_progress', /was in progress when this update was refused/],
  ['dispatch_recovery_required', /needed recovery when this update was refused/],
  ['stale_revision', /Preferences changed before this update/],
  ['admission_conflict', /Agent changed during the update/],
] as const)('can settle the exact request from a recorded %s refusal', (reason, message) => {
  const error = new ApiError(409, 'private', 'agent_operation_in_flight', undefined, undefined, {
    settings_update_reason: reason,
  });
  expect(settingsWriteFailure(error, true)).toMatchObject({
    outcome: 'rejected',
    refreshRequired: true,
    message: expect.stringMatching(message),
  });
});

it.each(['outcome_unconfirmed', 'unsupported_reason'])(
  'does not settle an uncertain request based on %s',
  (reason) => {
    const error = new ApiError(409, 'private', 'agent_operation_in_flight', undefined, undefined, {
      settings_update_reason: reason,
    });
    expect(settingsWriteFailure(error, true)).toMatchObject({
      outcome: 'unconfirmed',
      refreshRequired: true,
    });
  },
);

it('gives access refusal precedence over a claimed operation reason', () => {
  const error = new ApiError(403, 'private', 'agent_operation_in_flight', undefined, undefined, {
    settings_update_reason: 'dispatch_in_progress',
  });
  expect(settingsWriteFailure(error, true)).toMatchObject({
    outcome: 'unconfirmed',
    kind: 'denied',
  });
});
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
