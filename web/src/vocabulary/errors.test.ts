import { ApiError, ProtocolError, TransportError } from '@chronoai/cma-sdk';
import { readableError } from './errors';
import { DocumentError } from '../api/documents';

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

  it.each([
    ['document_limit_reached', 'document count limit'],
    ['document_bytes_limit_reached', 'retained document storage limit'],
    ['document_objects_limit_reached', 'stored document object limit'],
    ['document_write_busy', 'Document writing is busy'],
    ['document_deletion_unavailable', 'Document deletion is unavailable'],
  ])('explains typed document refusal %s before displaying technical details', (code, sentence) => {
    const message = readableError(new DocumentError('Technical detail', 503, code), 'fallback');
    expect(message).toContain(sentence);
    if (code.endsWith('_limit_reached')) expect(message).toContain('Delete old documents');
  });

  it('never displays invalid quota dimensions or negative remaining capacity', () => {
    const over = readableError(
      new DocumentError('Capacity', 429, 'document_limit_reached', {
        limit: 100,
        live: 104,
      }),
      'fallback',
    );
    expect(over).toContain('0 documents available');
    expect(over).toContain('104 / 100 used');
    const unknown = readableError(
      new DocumentError('Capacity', 429, 'document_bytes_limit_reached', {
        limit: NaN,
        live: 0,
      }),
      'fallback',
    );
    expect(unknown).not.toMatch(/NaN|available/);
  });
});
