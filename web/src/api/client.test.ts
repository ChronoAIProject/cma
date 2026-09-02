import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ProtocolError, TransportError } from '@chronoai/cma-sdk';
import {
  ApiError,
  createChatSession,
  createResponsesTurn,
  followSession,
  listResponsesConversationItems,
  listKernels,
  listSessions,
  listTriggerRevisions,
  listTriggers,
  readChatSurfaceEnabled,
  readResponsesConversation,
  readResponsesResponse,
  toSession,
} from './client';

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

describe('the Chat surface client boundary', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  function capability(custom: Record<string, unknown>): Response {
    return new Response(JSON.stringify({ protocol: 'ag-ui', features: { custom } }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  }

  it('enables Chat only for the exact live capability declaration', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(capability({ 'cma:chat': { enabled: true } }))
      .mockResolvedValueOnce(capability({ 'cma:chat': { enabled: false } }))
      .mockResolvedValueOnce(capability({ 'cma:chat': { enabled: 'true' } }))
      .mockResolvedValueOnce(capability({}))
      .mockRejectedValueOnce(new Error('offline'));
    vi.stubGlobal('fetch', fetch);

    await expect(readChatSurfaceEnabled()).resolves.toBe(true);
    await expect(readChatSurfaceEnabled()).resolves.toBe(false);
    await expect(readChatSurfaceEnabled()).resolves.toBe(false);
    await expect(readChatSurfaceEnabled()).resolves.toBe(false);
    await expect(readChatSurfaceEnabled()).resolves.toBe(false);
  });

  it('passes an exact surface filter through the reference SDK', async () => {
    const fetch = vi.fn().mockResolvedValue(
      new Response(JSON.stringify({ sessions: [valid] }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    );
    vi.stubGlobal('fetch', fetch);

    await expect(listSessions('agent')).resolves.toHaveLength(1);
    expect(fetch).toHaveBeenCalledWith(
      '/api/v1/sessions?surface=agent',
      expect.objectContaining({ method: 'GET' }),
    );
  });

  it('creates Chat directly with no profile, geometry, or price fields', async () => {
    const fetch = vi.fn().mockResolvedValue(
      new Response(
        JSON.stringify({
          ...valid,
          id: 'chat-new',
          chat: { worktree_name: 'chat-new', worktree_path: '/work/chat-new', title: null },
        }),
        { status: 201, headers: { 'content-type': 'application/json' } },
      ),
    );
    vi.stubGlobal('fetch', fetch);

    await expect(createChatSession()).resolves.toMatchObject({
      id: 'chat-new',
      chat: { worktreeName: 'chat-new', worktreePath: '/work/chat-new' },
    });
    const request = fetch.mock.calls[0]?.[1] as RequestInit | undefined;
    expect(request?.method).toBe('POST');
    expect(JSON.parse(String(request?.body))).toEqual({ surface: 'chat' });
  });

  it('binds every Chat Responses operation to the public same-origin SDK routes', async () => {
    const durableResponse = JSON.parse(
      readFileSync(
        resolve(
          process.cwd(),
          '../api/crates/layer1-public/tests/fixtures/responses-sdk/response.json',
        ),
        'utf8',
      ),
    ) as Record<string, unknown>;
    const fetch = vi.fn((input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url === '/api/v1/conversations/chat%2Fowner') {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              id: 'chat/owner',
              object: 'conversation',
              created_at: 1_788_228_800,
              metadata: {},
            }),
            { status: 200, headers: { 'content-type': 'application/json' } },
          ),
        );
      }
      if (url === '/api/v1/conversations/chat%2Fowner/items?after=msg%2F2&limit=25') {
        return Promise.resolve(
          new Response(
            JSON.stringify({
              object: 'list',
              data: [],
              first_id: null,
              last_id: null,
              has_more: false,
            }),
            { status: 200, headers: { 'content-type': 'application/json' } },
          ),
        );
      }
      if (url === '/api/v1/responses/resp%2Fowner') {
        return Promise.resolve(
          new Response(JSON.stringify({ ...durableResponse, id: 'resp/owner' }), {
            status: 200,
            headers: { 'content-type': 'application/json' },
          }),
        );
      }
      if (url === '/api/v1/responses' && init?.method === 'POST') {
        return Promise.resolve(
          new Response(new ReadableStream<Uint8Array>({ start() {} }), {
            status: 200,
            headers: { 'content-type': 'text/event-stream' },
          }),
        );
      }
      throw new Error(`unexpected request ${url}`);
    });
    vi.stubGlobal('fetch', fetch);
    const controller = new AbortController();

    await expect(readResponsesConversation('chat/owner', controller.signal)).resolves.toMatchObject(
      {
        id: 'chat/owner',
        object: 'conversation',
      },
    );
    await expect(
      listResponsesConversationItems(
        'chat/owner',
        { after: 'msg/2', limit: 25 },
        controller.signal,
      ),
    ).resolves.toMatchObject({ object: 'list', data: [] });
    await expect(readResponsesResponse('resp/owner', controller.signal)).resolves.toMatchObject({
      id: 'resp/owner',
      object: 'response',
    });
    const connection = await createResponsesTurn(
      { conversation: 'chat/owner', input: 'Inspect the release' },
      { signal: controller.signal },
    );

    expect(fetch.mock.calls.map(([input]) => String(input))).toEqual([
      '/api/v1/conversations/chat%2Fowner',
      '/api/v1/conversations/chat%2Fowner/items?after=msg%2F2&limit=25',
      '/api/v1/responses/resp%2Fowner',
      '/api/v1/responses',
    ]);
    expect(JSON.parse(String(fetch.mock.calls[3]?.[1]?.body))).toEqual({
      conversation: 'chat/owner',
      input: 'Inspect the release',
      stream: true,
    });
    for (const [, init] of fetch.mock.calls) {
      expect(init?.signal).toBeInstanceOf(AbortSignal);
    }
    connection.cancel('test complete');
  });
});

describe('publisher catalogues', () => {
  const launch = JSON.parse(
    readFileSync(resolve(process.cwd(), '../sdk/typescript/tests/fixtures/launch.json'), 'utf8'),
  ) as Record<string, unknown>;
  const content = launch.revision_content as Record<string, unknown>;

  function draft(index: number) {
    const id = `draft-${String(index).padStart(3, '0')}`;
    return {
      id,
      selected_service_id: 'service-fixture',
      version: 1,
      next_revision_number: 2,
      content: { ...content, trigger_id: id },
      created_at: '2026-08-27T00:00:00Z',
      updated_at: '2026-08-27T01:00:00Z',
    };
  }

  function revision(index: number) {
    const id = `revision-${String(index).padStart(3, '0')}`;
    return {
      id,
      public_id: `public-${String(index).padStart(3, '0')}`,
      digest: String(index).padStart(64, '0'),
      availability: 'published',
      content: { ...content, trigger_id: `draft-${String(index).padStart(3, '0')}` },
      authority: {
        service_id: 'service-fixture',
        service_state_version: 1,
        account_id: 42,
        account_login: 'contributor',
        role: 'owner',
        verified_at: '2026-08-27T00:00:00Z',
      },
      published_at: '2026-08-27T00:00:00Z',
      retired_at: null,
    };
  }

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('walks every bounded draft and revision cursor page', async () => {
    const fetch = vi.fn((input: RequestInfo | URL) => {
      const url = String(input);
      const revisions = url.includes('/api/v1/trigger-revisions');
      const second = url.includes('cursor=page-2');
      const items = Array.from({ length: second ? 10 : 50 }, (_, offset) => {
        const index = offset + (second ? 50 : 0);
        return revisions ? revision(index) : draft(index);
      });
      return Promise.resolve(
        new Response(JSON.stringify({ items, next_cursor: second ? null : 'page-2' }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        }),
      );
    });
    vi.stubGlobal('fetch', fetch);

    const [drafts, revisions] = await Promise.all([listTriggers(), listTriggerRevisions()]);

    expect(drafts).toHaveLength(60);
    expect(revisions).toHaveLength(60);
    expect(drafts[55]?.id).toBe('draft-055');
    expect(revisions[55]?.id).toBe('revision-055');
    expect(fetch.mock.calls.map(([input]) => String(input))).toEqual(
      expect.arrayContaining([
        '/api/v1/triggers?limit=200&cursor=page-2',
        '/api/v1/trigger-revisions?limit=200&cursor=page-2',
      ]),
    );
  });

  it('fails closed when cursor pagination makes no progress', async () => {
    vi.stubGlobal(
      'fetch',
      vi.fn().mockResolvedValue(
        new Response(JSON.stringify({ items: [], next_cursor: 'stuck' }), {
          status: 200,
          headers: { 'Content-Type': 'application/json' },
        }),
      ),
    );

    await expect(listTriggers()).rejects.toBeInstanceOf(ProtocolError);
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
