import { describe, expect, it } from 'vitest';
import { ApiError, toSession } from './client';

/**
 * The wire is a trust boundary. These cover what the control plane actually
 * sends and what a malformed payload must do — fail visibly rather than become
 * `undefined` somewhere in the UI.
 */

const valid = {
  id: 'sesn_abc',
  kernel: 'codex',
  status: 'ready',
  created_at: '2026-08-13T10:00:00Z',
};

describe('toSession', () => {
  it('maps the control plane payload onto the display model', () => {
    const s = toSession(valid);
    expect(s.id).toBe('sesn_abc');
    expect(s.kernel).toBe('codex');
    expect(s.lifecycle).toBe('ready');
    expect(s.createdAt).toBe(Date.parse('2026-08-13T10:00:00Z'));
  });

  it('carries the creator when the control plane names them', () => {
    const s = toSession({
      ...valid,
      creator: { display_name: 'Ada Lovelace', avatar_url: 'https://pictures.example/ada.png' },
    });
    expect(s.creator?.displayName).toBe('Ada Lovelace');
    expect(s.creator?.avatarUrl).toBe('https://pictures.example/ada.png');
  });

  it('leaves the creator absent when the control plane named nobody', () => {
    // A caller without the grant to name people is sent no creator at all, and
    // the panel must omit the row rather than draw a heading over nothing.
    expect(toSession(valid).creator).toBeUndefined();
    expect(toSession({ ...valid, creator: {} }).creator).toBeUndefined();
    expect(toSession({ ...valid, creator: null }).creator).toBeUndefined();
  });

  it('keeps a creator that has a name but no picture', () => {
    // Every directory entry written before pictures were captured is this
    // shape, so it must render rather than be discarded as malformed.
    const s = toSession({ ...valid, creator: { display_name: 'Ada Lovelace' } });
    expect(s.creator?.displayName).toBe('Ada Lovelace');
    expect(s.creator?.avatarUrl).toBeUndefined();
  });

  it('accepts every lifecycle the core models', () => {
    for (const status of ['provisioning', 'ready', 'running', 'idle', 'terminated']) {
      expect(toSession({ ...valid, status }).lifecycle).toBe(status);
    }
  });

  it('rejects a status the frontend does not model', () => {
    // Coercing this would render a session in a state the control plane never
    // reported, which is worse than refusing to render it.
    expect(() => toSession({ ...valid, status: 'exploded' })).toThrow(ApiError);
  });

  it('rejects an unrecognised kernel', () => {
    expect(() => toSession({ ...valid, kernel: 'something-else' })).toThrow(ApiError);
  });

  it('rejects a payload with no id', () => {
    expect(() => toSession({ ...valid, id: '' })).toThrow(ApiError);
    expect(() => toSession({ ...valid, id: undefined })).toThrow(ApiError);
  });

  it('falls back to now when the timestamp is unusable', () => {
    // The tab strip shows an age; "NaN ago" would be worse than approximate.
    const before = Date.now();
    const s = toSession({ ...valid, created_at: 'not-a-date' });
    expect(s.createdAt).toBeGreaterThanOrEqual(before);
  });
});

describe('ApiError', () => {
  it('distinguishes not-signed-in from other failures', () => {
    // The console reacts differently to these: one is a prompt to sign in,
    // the other is something to report.
    expect(new ApiError(401, 'nope').isUnauthenticated).toBe(true);
    expect(new ApiError(500, 'boom').isUnauthenticated).toBe(false);
    expect(new ApiError(0, 'offline').isUnauthenticated).toBe(false);
  });
});
