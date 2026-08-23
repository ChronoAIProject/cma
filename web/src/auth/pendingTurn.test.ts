import { beforeEach, describe, expect, it, vi } from 'vitest';
import {
  MAX_TURN_LENGTH,
  capturePendingTurn,
  clearPendingTurn,
  consumePendingTurn,
} from './pendingTurn';

const KEY = 'cma:pending-turn';
const LEGACY_KEY = 'chrono-bot:pending-turn';

function pending(text: string, capturedAt: number = Date.now()): string {
  return JSON.stringify({ text, capturedAt });
}

describe('pendingTurn', () => {
  beforeEach(() => {
    window.sessionStorage.clear();
    vi.restoreAllMocks();
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
    const raw = window.sessionStorage.getItem(KEY) ?? '';
    expect(Object.keys(JSON.parse(raw) as object).sort()).toEqual(['capturedAt', 'text']);
    expect(window.sessionStorage.getItem(LEGACY_KEY)).toBeNull();
    expect(window.localStorage.getItem(KEY)).toBeNull();
  });

  it('consumes a valid historical prompt once and clears both names', () => {
    window.sessionStorage.setItem(LEGACY_KEY, pending('historical'));

    expect(consumePendingTurn()).toBe('historical');
    expect(window.sessionStorage.getItem(KEY)).toBeNull();
    expect(window.sessionStorage.getItem(LEGACY_KEY)).toBeNull();
    expect(consumePendingTurn()).toBeNull();
  });

  it('prefers valid canonical content and clears both names', () => {
    window.sessionStorage.setItem(KEY, pending('canonical'));
    window.sessionStorage.setItem(LEGACY_KEY, pending('historical'));

    expect(consumePendingTurn()).toBe('canonical');
    expect(window.sessionStorage.getItem(KEY)).toBeNull();
    expect(window.sessionStorage.getItem(LEGACY_KEY)).toBeNull();
  });

  it('falls back to valid historical content when canonical content is malformed', () => {
    window.sessionStorage.setItem(KEY, 'not json');
    window.sessionStorage.setItem(LEGACY_KEY, pending('historical'));

    expect(consumePendingTurn()).toBe('historical');
    expect(window.sessionStorage.getItem(KEY)).toBeNull();
    expect(window.sessionStorage.getItem(LEGACY_KEY)).toBeNull();
  });

  it('discards and clears both names when neither contains valid content', () => {
    window.sessionStorage.setItem(KEY, 'not json');
    window.sessionStorage.setItem(LEGACY_KEY, '{"text":123}');

    expect(consumePendingTurn()).toBeNull();
    expect(window.sessionStorage.getItem(KEY)).toBeNull();
    expect(window.sessionStorage.getItem(LEGACY_KEY)).toBeNull();
  });

  it('discards a stale capture', () => {
    capturePendingTurn('yesterday', 0);
    // Two hours later, past the one-hour bound.
    expect(consumePendingTurn(2 * 60 * 60 * 1000)).toBeNull();
  });

  it('rejects malformed storage rather than trusting it', () => {
    // Anything on the origin can write here, so the value is untrusted input.
    for (const bad of ['not json', '{}', 'null', '[]', '{"text":123}', '{"text":"ok"}']) {
      window.sessionStorage.setItem(KEY, bad);
      expect(consumePendingTurn()).toBeNull();
    }
  });

  it('rejects a non-finite timestamp', () => {
    window.sessionStorage.setItem(KEY, JSON.stringify({ text: 'hi', capturedAt: Number.NaN }));
    expect(consumePendingTurn()).toBeNull();
  });

  it('drops a stored prompt on request', () => {
    capturePendingTurn('discard me');
    window.sessionStorage.setItem(LEGACY_KEY, pending('old discard'));
    clearPendingTurn();
    expect(window.sessionStorage.getItem(KEY)).toBeNull();
    expect(window.sessionStorage.getItem(LEGACY_KEY)).toBeNull();
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

  it('does not throw when clearing is blocked', () => {
    vi.spyOn(window.sessionStorage, 'removeItem').mockImplementation(() => {
      throw new DOMException('denied', 'SecurityError');
    });

    expect(() => clearPendingTurn()).not.toThrow();
  });
});
