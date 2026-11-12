import { ApiError, ProtocolError, TransportError } from '@chronoai/cma-sdk';
import { failureRemediation, readableError, refusalSentence } from './errors';

describe('public allowance guidance', () => {
  it('explains the daily allowance with its local reset time without exposing wire words', () => {
    const resetAt = '2026-11-13T00:00:00Z';
    const error = new ApiError(
      429,
      'unsafe Launch resource_quota',
      'resource_quota',
      undefined,
      undefined,
      {
        resource: 'visitor_launches',
        limit: 5,
        reset_at: resetAt,
      },
    );
    const sentence = refusalSentence(error);
    const formatted = new Intl.DateTimeFormat(undefined, {
      month: 'short',
      day: 'numeric',
      hour: 'numeric',
      minute: '2-digit',
    }).format(new Date(resetAt));

    expect(sentence).toContain(formatted);
    expect(sentence).toMatch(/daily allowance/i);
    expect(sentence).toMatch(/try again/i);
    expect(sentence).not.toMatch(
      /\b(launch|revision|digest|idempotency|reconcile|retained|capability|ceiling|checkpoint|reach|fence|userservice|mib|millis)\b|\b[a-z]+_[a-z_]+\b/i,
    );
  });

  it.each([undefined, null, '', 'unknown', 1794528000])(
    'keeps an uncertain result when the reset time is unusable: %j',
    (resetAt) => {
      const error = new ApiError(
        429,
        'unsafe Launch resource_quota',
        'resource_quota',
        undefined,
        undefined,
        {
          resource: 'visitor_launches',
          reset_at: resetAt,
        },
      );
      expect(refusalSentence(error)).toBe(refusalSentence(new Error('offline')));
    },
  );

  it('explains that a full Sandbox allowance needs a completed deletion before starting again', () => {
    const sentence = failureRemediation('sandbox_limit_reached');
    expect(sentence).toMatch(/delete.*sandbox/i);
    expect(sentence).toMatch(/wait.*deletion.*complete/i);
    expect(sentence).toMatch(/start again/i);
    expect(sentence).not.toMatch(/\b(launch|revision|idempotency)\b|\b[a-z]+_[a-z_]+\b/i);
  });
});

describe('readableError', () => {
  it('keeps a service sentence written for people', () => {
    expect(readableError(new ApiError(409, 'The Sandbox is pausing.'), 'fallback')).toBe(
      'The Sandbox is pausing.',
    );
  });

  it('replaces server-side failures with a plain sentence', () => {
    expect(readableError(new ApiError(502, 'upstream reset by peer'), 'fallback')).toBe(
      'CMA had a problem on its side. Try again in a moment.',
    );
  });

  it.each([
    [401, 'Your sign-in has expired. Reload the page to sign in again.'],
    [403, 'Your account does not have access to this.'],
    [404, 'This could not be found. It may have been deleted or moved.'],
    [429, 'CMA is receiving too many requests. Wait a moment, then try again.'],
    [500, 'CMA had a problem on its side. Try again in a moment.'],
  ])('explains an HTTP %i response that carried no detail', (status, sentence) => {
    const error = new ProtocolError(`HTTP ${String(status)} problem document carried no detail`);
    expect(readableError(error, 'fallback')).toBe(sentence);
  });

  it('uses the caller fallback for any other protocol fault', () => {
    expect(
      readableError(new ProtocolError('Agent field `id` is invalid'), 'Agents unavailable.'),
    ).toBe('Agents unavailable.');
  });

  it('says when CMA cannot be reached', () => {
    const sentence = 'CMA could not be reached. Check your connection, then try again.';
    expect(readableError(new TransportError('socket hang up'), 'fallback')).toBe(sentence);
    expect(readableError(new TypeError('Failed to fetch'), 'fallback')).toBe(sentence);
  });

  it('keeps other errors and falls back when there is nothing to say', () => {
    expect(readableError(new Error('Draft could not be saved.'), 'fallback')).toBe(
      'Draft could not be saved.',
    );
    expect(readableError('nope', 'fallback')).toBe('fallback');
  });
});
