import { afterEach, describe, expect, it, vi } from 'vitest';
import { ProtocolError, TransportError } from '@chronoai/cma-sdk';
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

  it('preserves a status the frontend does not model', () => {
    const session = toSession({ ...valid, status: 'exploded' });

    expect(session.lifecycle).toBe('unknown');
    expect(session.reportedStatus).toBe('exploded');
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
    expect(() => toSession({ ...valid, kernel: '' })).toThrow(ProtocolError);
    expect(() => toSession({ ...valid, kernel: undefined })).toThrow(ProtocolError);
  });

  it('rejects a payload with no id', () => {
    expect(() => toSession({ ...valid, id: '' })).toThrow(ProtocolError);
    expect(() => toSession({ ...valid, id: undefined })).toThrow(ProtocolError);
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
    expect(new TransportError('offline')).toBeInstanceOf(TransportError);
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
      vi.fn().mockResolvedValue(
        new Response(JSON.stringify(body), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        }),
      ),
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

    await expect(listKernels()).rejects.toBeInstanceOf(ProtocolError);
  });

  it('fails visibly when the answer is not a list', async () => {
    answer({ kernels: 'codex' });

    await expect(listKernels()).rejects.toBeInstanceOf(ProtocolError);
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
    readonly #encoder = new TextEncoder();
    readonly #controller: ReadableStreamDefaultController<Uint8Array>;
    readonly response: Response;
    #sequence = 0;

    constructor() {
      let controller: ReadableStreamDefaultController<Uint8Array> | undefined;
      const body = new ReadableStream<Uint8Array>({
        start(value) {
          controller = value;
        },
      });
      if (controller === undefined) {
        throw new Error('the response stream was not initialised');
      }
      this.#controller = controller;
      this.response = new Response(body, {
        status: 200,
        headers: { 'Content-Type': 'text/event-stream' },
      });
    }

    deliver(event: unknown): void {
      this.#sequence += 1;
      const data = JSON.stringify({ seq: this.#sequence, session_id: 'sesn_abc', event });
      this.#controller.enqueue(
        this.#encoder.encode(`id: ${String(this.#sequence)}\ndata: ${data}\n\n`),
      );
    }

    abort(): void {
      try {
        this.#controller.error(new DOMException('aborted', 'AbortError'));
      } catch {
        // The stream was already closed by the parser or an earlier abort.
      }
    }
  }

  function following() {
    const stream = new Stream();
    const fetchMock = vi
      .fn()
      .mockImplementation((_input: RequestInfo | URL, init?: RequestInit) => {
        init?.signal?.addEventListener('abort', () => stream.abort(), { once: true });
        return Promise.resolve(stream.response);
      });
    vi.stubGlobal('fetch', fetchMock);
    const signIns: unknown[] = [];
    const statuses: unknown[] = [];
    const stages: unknown[] = [];
    const errors: string[] = [];
    const stop = followSession(
      'sesn_abc',
      (status, detail) => statuses.push({ status, detail }),
      (message) => errors.push(message),
      (stage) => stages.push(stage),
      (signIn) => signIns.push(signIn),
    );
    return { errors, fetchMock, signIns, stages, statuses, stop, stream };
  }

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('reports the address and the code its owner has to enter', async () => {
    const { stream, signIns, stop } = following();

    stream.deliver({
      type: 'sign_in_required',
      url: 'https://auth.example.test/device',
      code: 'WDJB-MJHT',
      message: 'Sign in to continue',
    });
    await vi.waitFor(() => expect(signIns).toHaveLength(1));
    stop();

    expect(signIns).toEqual([
      {
        url: 'https://auth.example.test/device',
        code: 'WDJB-MJHT',
        message: 'Sign in to continue',
      },
    ]);
  });

  it('reports a request that carries no code, since the address is the whole of it', async () => {
    // Not every device flow shows a code on the asking end, and refusing the
    // request for want of one would leave the session waiting on nothing.
    const { stream, signIns, stop } = following();

    stream.deliver({ type: 'sign_in_required', url: 'https://auth.example.test/device' });
    await vi.waitFor(() => expect(signIns).toHaveLength(1));
    stop();

    expect(signIns).toEqual([
      { url: 'https://auth.example.test/device', code: undefined, message: undefined },
    ]);
  });

  it('reports a malformed request with no address', async () => {
    const { errors, stream, signIns, stop } = following();

    stream.deliver({ type: 'sign_in_required', code: 'WDJB-MJHT' });
    await vi.waitFor(() => expect(errors).toHaveLength(1));
    stop();

    expect(signIns).toEqual([]);
    expect(errors[0]).toMatch(/sign_in_required url/i);
  });

  it('reports the frame production actually sends, with a null code', async () => {
    /*
     * Copied from a production session's durable log. Codex declares no
     * separate code and puts it inside the sentence, so the field arrives as
     * `null` — which is not `undefined`, and passing it through drew an empty
     * code block above a Copy control that copied nothing.
     */
    const { stream, signIns, stop } = following();

    stream.deliver({
      type: 'sign_in_required',
      url: 'https://auth.openai.com/codex/device',
      code: null,
      message: 'Sign in to ChatGPT and enter this code: 1YF1-RUC10',
    });
    await vi.waitFor(() => expect(signIns).toHaveLength(1));
    stop();

    expect(signIns).toEqual([
      {
        url: 'https://auth.openai.com/codex/device',
        code: undefined,
        message: 'Sign in to ChatGPT and enter this code: 1YF1-RUC10',
      },
    ]);
  });

  it('treats an empty code as no code rather than as a code', async () => {
    // An empty string draws the same empty block a null did.
    const { stream, signIns, stop } = following();

    stream.deliver({ type: 'sign_in_required', url: 'https://auth.example.test/device', code: '' });
    await vi.waitFor(() => expect(signIns).toHaveLength(1));
    stop();

    expect(signIns[0]).toMatchObject({ code: undefined });
  });

  it('projects progress and lifecycle frames without inventing a sign-in', async () => {
    const { stream, signIns, stages, statuses, stop } = following();

    stream.deliver({ type: 'progress', stage: 'attaching' });
    stream.deliver({ type: 'status_changed', status: 'ready' });
    await vi.waitFor(() => expect(statuses).toHaveLength(1));
    stop();

    expect(signIns).toEqual([]);
    expect(stages).toEqual(['attaching']);
    expect(statuses).toEqual([{ status: 'ready', detail: undefined }]);
  });

  it('aborts the SDK stream without reporting cancellation as a failure', async () => {
    const { errors, fetchMock, stop } = following();
    await vi.waitFor(() => expect(fetchMock).toHaveBeenCalledOnce());

    stop();
    await Promise.resolve();

    expect(errors).toEqual([]);
  });
});
