import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError, followSession, listKernels, toSession } from './client';

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

  it('keeps a session on a runtime this build has never heard of', () => {
    // The defect this replaces: an allowlist of kernel identifiers threw on
    // anything else, and one such session failed the whole list read — so a
    // single session on an unfamiliar runtime hid every other session the
    // owner had. Which runtimes a deployment offers is a fact about that
    // deployment; the client draws an unfamiliar one under its own name.
    expect(toSession({ ...valid, kernel: 'dsh' }).kernel).toBe('dsh');
    expect(toSession({ ...valid, kernel: 'claude_code' }).kernel).toBe('claude_code');
    expect(toSession({ ...valid, kernel: 'something-else' }).kernel).toBe('something-else');
  });

  it('still refuses a session that names no runtime at all', () => {
    // Absent is not unfamiliar: nothing can be drawn for it.
    expect(() => toSession({ ...valid, kernel: '' })).toThrow(ApiError);
    expect(() => toSession({ ...valid, kernel: undefined })).toThrow(ApiError);
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

describe('what a session reaches', () => {
  it('tells an absent field apart from an empty one', () => {
    // "We were not told" is not "this session reaches nothing", and a client
    // that collapsed them would answer a question nobody asked.
    const absent = toSession({
      id: 'sesn_1',
      status: 'running',
      kernel: 'codex',
      created_at: '2026-08-18T08:53:00Z',
    });
    expect(absent.reach).toBeUndefined();

    const empty = toSession({
      id: 'sesn_1',
      status: 'running',
      kernel: 'codex',
      created_at: '2026-08-18T08:53:00Z',
      reach: [],
    });
    expect(empty.reach).toEqual([]);
  });

  it('keeps the identifier when the catalogue offered no name', () => {
    const session = toSession({
      id: 'sesn_1',
      status: 'running',
      kernel: 'codex',
      created_at: '2026-08-18T08:53:00Z',
      reach: [{ id: 'api-github', mandated: false }],
    });
    expect(session.reach).toEqual([
      { id: 'api-github', name: undefined, description: undefined, mandated: false },
    ]);
  });

  it('drops an entry with no usable identifier rather than drawing a blank one', () => {
    const session = toSession({
      id: 'sesn_1',
      status: 'running',
      kernel: 'codex',
      created_at: '2026-08-18T08:53:00Z',
      reach: [{ name: 'GitHub' }, null, 'api-github', { id: '', mandated: true }],
    });
    expect(session.reach).toEqual([]);
  });

  it('reads a missing mandated flag as not mandated', () => {
    // The safe direction: drawing a chosen service as fixed would tell
    // somebody they cannot remove something they can.
    const session = toSession({
      id: 'sesn_1',
      status: 'running',
      kernel: 'codex',
      created_at: '2026-08-18T08:53:00Z',
      reach: [{ id: 'api-github' }],
    });
    expect(session.reach?.[0]?.mandated).toBe(false);
  });
});

describe('listKernels', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function answer(body: unknown): void {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue({
        ok: true,
        status: 200,
        json: () => Promise.resolve(body),
      } as unknown as Response),
    );
  }

  it('reads what each runtime can be', async () => {
    answer({
      kernels: [
        { id: 'codex', modes: ['terminal', 'conversation'] },
        { id: 'claude-code', modes: ['conversation'] },
      ],
    });

    await expect(listKernels()).resolves.toEqual([
      { id: 'codex', modes: ['terminal', 'conversation'] },
      { id: 'claude-code', modes: ['conversation'] },
    ]);
  });

  it('drops a plane this client cannot draw', async () => {
    // Offering it would put a choice on screen that ends in a surface nobody
    // has written. The runtime is still listed, because it still exists.
    answer({ kernels: [{ id: 'future', modes: ['terminal', 'holodeck'] }] });

    await expect(listKernels()).resolves.toEqual([{ id: 'future', modes: ['terminal'] }]);
  });

  it('keeps a runtime that serves nothing', async () => {
    // "Runs no sessions" and "does not exist" are different answers, and a
    // form draws them differently.
    answer({ kernels: [{ id: 'stub', modes: [] }] });

    await expect(listKernels()).resolves.toEqual([{ id: 'stub', modes: [] }]);
  });

  it('fails visibly on a runtime with no identifier', async () => {
    answer({ kernels: [{ modes: ['terminal'] }] });

    await expect(listKernels()).rejects.toBeInstanceOf(ApiError);
  });

  it('fails visibly when the answer is not a list', async () => {
    answer({ kernels: 'codex' });

    await expect(listKernels()).rejects.toBeInstanceOf(ApiError);
  });
});

/**
 * The sign-in frame — #355.
 *
 * A kernel spending its owner's own vendor account stops inside the
 * conversation open to ask them to sign in, so this frame always arrives while
 * the session is still provisioning. That is the one state whose surface is
 * drawn from this stream rather than from the conversation, which is why the
 * stream has to carry it at all.
 *
 * Driven as the frame the control plane actually serialises, not as a prop: it
 * was already arriving and being dropped, and only the wire shape shows that.
 */
describe('followSession and the sign-in frame', () => {
  class Stream {
    static readonly CLOSED = 2;
    static last: Stream | null = null;
    readyState = 1;
    onmessage: ((e: MessageEvent) => void) | null = null;
    onerror: (() => void) | null = null;
    constructor(readonly url: string) {
      Stream.last = this;
    }
    deliver(event: unknown): void {
      this.onmessage?.({ data: JSON.stringify({ seq: 7, event }) } as MessageEvent);
    }
    close(): void {
      this.readyState = Stream.CLOSED;
    }
  }

  function following() {
    vi.stubGlobal('EventSource', Stream);
    const signIns: unknown[] = [];
    const stop = followSession(
      'sesn_abc',
      () => {},
      () => {},
      undefined,
      (signIn) => signIns.push(signIn),
    );
    const stream = Stream.last;
    if (stream === null) {
      throw new Error('the stream was never opened');
    }
    return { stream, signIns, stop };
  }

  afterEach(() => {
    vi.unstubAllGlobals();
    Stream.last = null;
  });

  it('reports the address and the code its owner has to enter', () => {
    const { stream, signIns, stop } = following();

    stream.deliver({
      type: 'sign_in_required',
      url: 'https://auth.example.test/device',
      code: 'WDJB-MJHT',
      message: 'Sign in to continue',
    });
    stop();

    expect(signIns).toEqual([
      {
        url: 'https://auth.example.test/device',
        code: 'WDJB-MJHT',
        message: 'Sign in to continue',
      },
    ]);
  });

  it('reports a request that carries no code, since the address is the whole of it', () => {
    // Not every device flow shows a code on the asking end, and refusing the
    // request for want of one would leave the session waiting on nothing.
    const { stream, signIns, stop } = following();

    stream.deliver({ type: 'sign_in_required', url: 'https://auth.example.test/device' });
    stop();

    expect(signIns).toEqual([
      { url: 'https://auth.example.test/device', code: undefined, message: undefined },
    ]);
  });

  it('ignores a request with no address, which nobody could act on', () => {
    const { stream, signIns, stop } = following();

    stream.deliver({ type: 'sign_in_required', code: 'WDJB-MJHT' });
    stream.deliver({ type: 'sign_in_required', url: '', code: 'WDJB-MJHT' });
    stop();

    expect(signIns).toEqual([]);
  });

  it('leaves every other frame alone', () => {
    const { stream, signIns, stop } = following();

    stream.deliver({ type: 'progress', stage: 'attaching' });
    stream.deliver({ type: 'status_changed', status: 'ready' });
    stop();

    expect(signIns).toEqual([]);
  });
});
