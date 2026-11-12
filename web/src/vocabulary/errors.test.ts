import { ApiError, ProtocolError, TransportError } from '@chronoai/cma-sdk';
import { readableError, refusalSentence } from './errors';

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

it.each([
  [
    'account_deletion_in_progress',
    'Your CMA account deletion is already in progress. Check cleanup progress below.',
  ],
  ['account_deletion_unavailable', 'Account deletion is temporarily unavailable. Try again later.'],
  [
    'account_deletion_acknowledgement_required',
    'Acknowledge the irreversible CMA data deletion before continuing.',
  ],
  [
    'account_deletion_requires_owner_session',
    'Sign in as the CMA account owner to request account deletion.',
  ],
  [
    'share_recipient_unavailable',
    'This share recipient is unavailable while their account is being deleted.',
  ],
] as const)('renders the %s problem sentence', (code, sentence) => {
  expect(refusalSentence(new ApiError(409, 'server detail', code))).toBe(sentence);
});
