import { describe, expect, it } from 'vitest';
import {
  isSendableGeometry,
  parseServerControl,
  pingFrame,
  resizeFrame,
  signalFrame,
  terminalUrl,
} from './protocol';

describe('server control parsing', () => {
  it('parses a ready frame with its advertised limits', () => {
    const msg = parseServerControl(
      JSON.stringify({
        t: 'ready',
        cols: 120,
        rows: 40,
        snapshot: true,
        conformsTo: ['https://example/conf/pty-core'],
        limits: { scrollbackLines: 2000, maxFrameBytes: 262144 },
      }),
    );
    expect(msg?.t).toBe('ready');
    if (msg?.t === 'ready') {
      expect(msg.limits.scrollbackLines).toBe(2000);
      expect(msg.cols).toBe(120);
    }
  });

  it('parses error and exit frames', () => {
    expect(
      parseServerControl(JSON.stringify({ t: 'error', code: 'already_attached', detail: 'x' }))?.t,
    ).toBe('error');
    expect(parseServerControl(JSON.stringify({ t: 'exit', code: 0, reason: 'end_turn' }))?.t).toBe(
      'exit',
    );
  });

  it('returns null for malformed json rather than throwing', () => {
    expect(parseServerControl('not json at all')).toBeNull();
    expect(parseServerControl('')).toBeNull();
  });

  it('returns null for an unknown control tag', () => {
    // Forward compatibility: an unrecognised control message must be skipped,
    // not treated as fatal.
    expect(parseServerControl(JSON.stringify({ t: 'something_new' }))).toBeNull();
    expect(parseServerControl(JSON.stringify({ nope: 1 }))).toBeNull();
    expect(parseServerControl('null')).toBeNull();
  });
});

describe('client frames', () => {
  it('builds a resize frame with the documented shape', () => {
    expect(JSON.parse(resizeFrame(80, 24))).toEqual({ t: 'resize', cols: 80, rows: 24 });
  });

  it('builds signal and ping frames', () => {
    expect(JSON.parse(signalFrame('INT'))).toEqual({ t: 'signal', sig: 'INT' });
    expect(JSON.parse(pingFrame())).toEqual({ t: 'ping' });
  });
});

describe('geometry guarding', () => {
  it('accepts a real geometry', () => {
    expect(isSendableGeometry(80, 24)).toBe(true);
  });

  it('rejects a zero dimension, which is a routine layout transient', () => {
    expect(isSendableGeometry(0, 24)).toBe(false);
    expect(isSendableGeometry(80, 0)).toBe(false);
    expect(isSendableGeometry(0, 0)).toBe(false);
  });

  it('rejects non-finite values from a failed fit calculation', () => {
    expect(isSendableGeometry(NaN, 24)).toBe(false);
    expect(isSendableGeometry(80, Infinity)).toBe(false);
  });
});

describe('terminal url', () => {
  it('upgrades http to ws and https to wss', () => {
    expect(terminalUrl('http://localhost:8080', 'sesn_1')).toBe(
      'ws://localhost:8080/api/v1/sessions/sesn_1/terminal',
    );
    expect(terminalUrl('https://api.example', 'sesn_2')).toBe(
      'wss://api.example/api/v1/sessions/sesn_2/terminal',
    );
  });

  it('tolerates a trailing slash on the base url', () => {
    expect(terminalUrl('http://localhost:8080///', 'sesn_1')).toBe(
      'ws://localhost:8080/api/v1/sessions/sesn_1/terminal',
    );
  });

  it('encodes the session id', () => {
    expect(terminalUrl('http://x', 'a/b')).toContain('a%2Fb');
  });
});
