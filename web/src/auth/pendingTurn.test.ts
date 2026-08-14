import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  MAX_TURN_LENGTH,
  capturePendingTurn,
  clearPendingTurn,
  consumePendingTurn,
} from './pendingTurn';

describe('pendingTurn', () => {
  beforeEach(() => {
    window.sessionStorage.clear();
  });

  it('carries a prompt across the sign-in round trip', () => {
    capturePendingTurn('build me a web scraper');
    expect(consumePendingTurn()).toBe('build me a web scraper');
  });

  it('clears on read so a refresh cannot resubmit it', () => {
    capturePendingTurn('deploy to production');

    expect(consumePendingTurn()).toBe('deploy to production');
    expect(consumePendingTurn()).toBeNull();
  });

  it('rejects an empty or whitespace-only prompt', () => {
    expect(capturePendingTurn('')).toBeNull();
    expect(capturePendingTurn('   \n\t ')).toBeNull();
    expect(consumePendingTurn()).toBeNull();
  });

  it('trims surrounding whitespace', () => {
    capturePendingTurn('  fix the flaky test  ');
    expect(consumePendingTurn()).toBe('fix the flaky test');
  });

  it('clips an overlong prompt at the boundary', () => {
    const captured = capturePendingTurn('x'.repeat(MAX_TURN_LENGTH + 500));
    expect(captured).toHaveLength(MAX_TURN_LENGTH);
    expect(consumePendingTurn()).toHaveLength(MAX_TURN_LENGTH);
  });

  it('never writes a token — only the prompt text and a timestamp', () => {
    capturePendingTurn('hello');
    const raw = window.sessionStorage.getItem('chrono-bot:pending-turn') ?? '';
    expect(Object.keys(JSON.parse(raw) as object).sort()).toEqual(['capturedAt', 'text']);
  });

  it('discards a stale capture', () => {
    capturePendingTurn('yesterday', 0);
    // Two hours later, past the one-hour bound.
    expect(consumePendingTurn(2 * 60 * 60 * 1000)).toBeNull();
  });

  it('rejects malformed storage rather than trusting it', () => {
    // Anything on the origin can write here, so the value is untrusted input.
    for (const bad of ['not json', '{}', 'null', '[]', '{"text":123}', '{"text":"ok"}']) {
      window.sessionStorage.setItem('chrono-bot:pending-turn', bad);
      expect(consumePendingTurn()).toBeNull();
    }
  });

  it('rejects a non-finite timestamp', () => {
    window.sessionStorage.setItem(
      'chrono-bot:pending-turn',
      JSON.stringify({ text: 'hi', capturedAt: Number.NaN }),
    );
    expect(consumePendingTurn()).toBeNull();
  });

  it('drops a stored prompt on request', () => {
    capturePendingTurn('discard me');
    clearPendingTurn();
    expect(consumePendingTurn()).toBeNull();
  });

  it('still returns the text when storage is unavailable', () => {
    // Safari private mode and some webviews throw on access rather than
    // merely being empty. Losing the prompt is acceptable; crashing is not.
    const spy = vi.spyOn(window.sessionStorage, 'setItem').mockImplementation(() => {
      throw new Error('QuotaExceededError');
    });

    expect(capturePendingTurn('resilient')).toBe('resilient');

    spy.mockRestore();
  });

  it('returns null rather than throwing when reading fails', () => {
    const spy = vi.spyOn(window.sessionStorage, 'getItem').mockImplementation(() => {
      throw new Error('SecurityError');
    });

    expect(consumePendingTurn()).toBeNull();

    spy.mockRestore();
  });
});
