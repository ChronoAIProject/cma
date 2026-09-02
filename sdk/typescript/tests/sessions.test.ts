import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { Cma } from '../src/client.js';
import { resolveConfig, type ResponseMetadata } from '../src/config.js';
import {
  ApiError,
  ProtocolError,
  RequestAborted,
  TimeoutError,
  ValidationError,
} from '../src/errors.js';
import {
  STOP_REASONS,
  changePlane,
  createSession,
  deleteSession,
  fetchTranscript,
  listKernels,
  listSessionPage,
  listSessions,
  quoteSession,
  readAllowance,
  readSession,
  readSpend,
  reviveSession,
  stopSession,
  terminateSession,
  toPricingQuote,
  toSession,
  toSessionSpend,
} from '../src/sessions.js';

/** @vitest-environment node */

function answering(body: unknown, status = 200) {
  const fetch = vi.fn(() =>
    Promise.resolve(
      new Response(JSON.stringify(body), {
        status,
        headers: { 'content-type': 'application/json' },
      }),
    ),
  );
  return { fetch: fetch as unknown as typeof globalThis.fetch, calls: fetch };
}

function fixture(name: string): unknown {
  return JSON.parse(readFileSync(new URL(`fixtures/${name}`, import.meta.url), 'utf8')) as unknown;
}

function sessionFixture(overrides: Record<string, unknown> = {}): Record<string, unknown> {
  return { ...(fixture('session.json') as Record<string, unknown>), ...overrides };
}

const KNOWN_LIFECYCLES = [
  'provisioning',
  'ready',
  'running',
  'rotating',
  'idle',
  'terminated',
] as const;

const RECOVERY_STATES = [
  'normal',
  'idle',
  'recovering',
  'degraded',
  'unknown',
  'retired',
  'invalid',
] as const;

function serializedEnumWords(name: string): readonly string[] {
  const source = readFileSync(
    new URL('../../../api/crates/layer2-core/src/session.rs', import.meta.url),
    'utf8',
  );
  const body = new RegExp(`pub enum ${name} \\{([\\s\\S]*?)\\n\\}`).exec(source)?.[1];
  if (body === undefined) {
    throw new Error(`layer2-core no longer declares ${name}`);
  }
  return [...body.matchAll(/^ {4}([A-Z][A-Za-z0-9]*),$/gm)].map(([, variant]) =>
    (variant ?? '').replace(/([a-z0-9])([A-Z])/g, '$1_$2').toLowerCase(),
  );
}

describe('the shared session fixture', () => {
  it('keeps every client-visible field, including runtime size and resume cursor', () => {
    const session = toSession(fixture('session.json'));

    expect(session).toMatchObject({
      id: 'sesn_fixture',
      kernel: 'codex',
      lifecycle: 'ready',
      recoveryState: 'normal',
      lastSeq: 7,
      cols: 120,
      rows: 40,
      plane: 'conversation',
      chat: {
        worktreeName: 'chat-3',
        worktreePath: '/workspace/chat-3',
        title: 'Fixture chat',
        pausedNotice: 'Paused after inactivity',
      },
      creator: {
        displayName: 'Fixture User',
        avatarUrl: 'https://images.example.test/fixture.png',
      },
      agent: {
        model: 'gpt-5',
        permission: 'guarded',
        reasoningEffort: 'high',
        credential: 'delegated',
        size: { cpuMillis: 1000, memoryMib: 2048 },
        skills: [
          {
            id: 'review',
            version: '1.2.3',
            name: 'Repository review',
            description: 'Review a repository without changing it',
          },
        ],
      },
    });
    expect(session.capabilities).toMatchObject({
      conversation: true,
      interrupt: true,
      usage: true,
    });
    expect(session.reach).toEqual([
      {
        id: 'github',
        name: 'GitHub',
        description: 'Source repositories',
        mandated: false,
      },
    ]);
  });

  it('reads both wrapped and bare session lists and an encoded session address', async () => {
    const bare = answering(fixture('sessions.json'));
    await expect(listSessions(resolveConfig({ fetch: bare.fetch }))).resolves.toHaveLength(1);

    const wrapped = answering({ sessions: fixture('sessions.json') });
    await expect(listSessions(resolveConfig({ fetch: wrapped.fetch }))).resolves.toHaveLength(1);

    const one = answering(fixture('session.json'));
    await expect(
      readSession(resolveConfig({ fetch: one.fetch }), 'sesn/fixture'),
    ).resolves.toMatchObject({
      id: 'sesn_fixture',
    });
    const [url] = one.calls.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toBe('/api/v1/sessions/sesn%2Ffixture');
  });

  it('finds next across Link fields and accepts unrelated links on the final page', async () => {
    const firstHeaders = new Headers({ 'content-type': 'application/json' });
    firstHeaders.append(
      'link',
      '<https://spec.chrono-ai.fun/cma/1.0/conf/rest-core>; rel="profile"',
    );
    firstHeaders.append('link', '</api/v1/sessions?limit=50&cursor=opaque>; rel="next"');
    const finalHeaders = new Headers({ 'content-type': 'application/json' });
    finalHeaders.append(
      'link',
      '<https://spec.chrono-ai.fun/cma/1.0/conf/rest-core>; rel="profile"',
    );
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify([sessionFixture({ id: 'sesn_newer' })]), {
          status: 200,
          headers: firstHeaders,
        }),
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify([sessionFixture({ id: 'sesn_older' })]), {
          status: 200,
          headers: finalHeaders,
        }),
      );

    const sessions = await listSessions(
      resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch }),
    );

    expect(sessions.map(({ id }) => id)).toEqual(['sesn_newer', 'sesn_older']);
    expect(fetch.mock.calls.map(([url]) => String(url))).toEqual([
      '/api/v1/sessions',
      '/api/v1/sessions?limit=50&cursor=opaque',
    ]);
  });

  it('reuses one request id and observes every response in a page walk', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify([sessionFixture({ id: 'sesn_page_one' })]), {
          status: 200,
          headers: {
            'content-type': 'application/json',
            link: '</api/v1/sessions?cursor=page-two>; rel="next"',
            'x-request-id': 'response-page-one',
          },
        }),
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify([sessionFixture({ id: 'sesn_page_two' })]), {
          status: 200,
          headers: {
            'content-type': 'application/json',
            'x-request-id': 'response-page-two',
          },
        }),
      );
    const requestIdProvider = vi.fn(() => 'caller-page-walk-488');
    const observed: ResponseMetadata[] = [];
    const config = resolveConfig({
      fetch: fetch as unknown as typeof globalThis.fetch,
      requestIdProvider,
      responseObserver: (metadata) => {
        observed.push(metadata);
      },
    });

    const sessions = await listSessions(config);

    expect(sessions.map(({ id }) => id)).toEqual(['sesn_page_one', 'sesn_page_two']);
    expect(requestIdProvider).toHaveBeenCalledOnce();
    expect(
      fetch.mock.calls.map(([, init]) => new Headers(init?.headers).get('x-request-id')),
    ).toEqual(['caller-page-walk-488', 'caller-page-walk-488']);
    expect(observed).toEqual([
      { status: 200, requestId: 'response-page-one' },
      { status: 200, requestId: 'response-page-two' },
    ]);
  });

  it('walks one exact surface and preserves the filter in every continuation', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify([sessionFixture({ id: 'chat_newer' })]), {
          status: 200,
          headers: {
            'content-type': 'application/json',
            link: '</api/v1/sessions?surface=chat&cursor=older>; rel="next"',
          },
        }),
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify([sessionFixture({ id: 'chat_older' })]), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
      );
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    const sessions = await listSessions(config, { surface: 'chat' });

    expect(sessions.map(({ id }) => id)).toEqual(['chat_newer', 'chat_older']);
    expect(fetch.mock.calls.map(([url]) => String(url))).toEqual([
      '/api/v1/sessions?surface=chat',
      '/api/v1/sessions?surface=chat&cursor=older',
    ]);
  });

  it('refuses an unknown complete-walk surface before transport', async () => {
    const fetch = vi.fn();
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    await expect(listSessions(config, { surface: 'workspace' as 'chat' })).rejects.toBeInstanceOf(
      ValidationError,
    );
    expect(fetch).not.toHaveBeenCalled();
  });

  it('exposes bounded status pages whose opaque next target can be followed manually', async () => {
    const fetch = vi
      .fn()
      .mockResolvedValueOnce(
        new Response(JSON.stringify([sessionFixture({ id: 'sesn_running_newer' })]), {
          status: 200,
          headers: {
            'content-type': 'application/json',
            link: '</api/v1/sessions?status=running&surface=chat&limit=1&cursor=opaque-next>; rel="next"',
          },
        }),
      )
      .mockResolvedValueOnce(
        new Response(JSON.stringify([sessionFixture({ id: 'sesn_running_older' })]), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
      );
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    const first = await listSessionPage(config, {
      status: 'running',
      surface: 'chat',
      limit: 1,
      cursor: 'opaque start',
    });
    expect(first.items.map(({ id }) => id)).toEqual(['sesn_running_newer']);
    const next = first.next;
    expect(next).toBeDefined();
    if (next === undefined) {
      throw new Error('first page did not carry its continuation');
    }
    await expect(listSessionPage(config, { next, status: 'running' })).rejects.toBeInstanceOf(
      ValidationError,
    );
    await expect(listSessionPage(config, { next, surface: 'agent' })).rejects.toBeInstanceOf(
      ValidationError,
    );
    expect(fetch).toHaveBeenCalledTimes(1);
    const second = await listSessionPage(config, { next });
    expect(second.items.map(({ id }) => id)).toEqual(['sesn_running_older']);
    expect(second.next).toBeUndefined();
    expect(fetch.mock.calls.map(([url]) => String(url))).toEqual([
      '/api/v1/sessions?status=running&surface=chat&limit=1&cursor=opaque+start',
      '/api/v1/sessions?status=running&surface=chat&limit=1&cursor=opaque-next',
    ]);
  });

  it('exposes bounded session pages through the configured client', async () => {
    const fetch = vi.fn().mockResolvedValue(
      new Response(JSON.stringify([sessionFixture({ id: 'sesn_configured' })]), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    );
    const cma = new Cma({ fetch: fetch as unknown as typeof globalThis.fetch });

    const page = await cma.listSessionPage({ limit: 1 });

    expect(page.items.map(({ id }) => id)).toEqual(['sesn_configured']);
    expect(fetch).toHaveBeenCalledWith(
      '/api/v1/sessions?limit=1',
      expect.objectContaining({ method: 'GET' }),
    );
  });

  it('rejects manual page bounds before making an HTTP request', async () => {
    const fetch = vi.fn();
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    for (const limit of [0, 201, 1.5]) {
      await expect(listSessionPage(config, { limit })).rejects.toBeInstanceOf(ValidationError);
    }
    await expect(listSessionPage(config, { cursor: '' })).rejects.toBeInstanceOf(ValidationError);
    await expect(
      listSessionPage(config, { surface: 'workspace' as 'chat' }),
    ).rejects.toBeInstanceOf(ValidationError);
    expect(fetch).not.toHaveBeenCalled();
  });

  it('bounds compatibility walks by both pages and retained items', async () => {
    const continuing = () =>
      new Response(JSON.stringify([sessionFixture({ id: 'sesn_one' })]), {
        status: 200,
        headers: {
          'content-type': 'application/json',
          link: '</api/v1/sessions?cursor=another>; rel="next"',
        },
      });
    const pageFetch = vi.fn().mockResolvedValue(continuing());
    await expect(
      listSessions(resolveConfig({ fetch: pageFetch as unknown as typeof globalThis.fetch }), {
        maxPages: 1,
      }),
    ).rejects.toThrow(/1-page boundary/i);
    expect(pageFetch).toHaveBeenCalledTimes(1);

    const itemFetch = vi
      .fn()
      .mockResolvedValue(
        new Response(
          JSON.stringify([sessionFixture({ id: 'sesn_one' }), sessionFixture({ id: 'sesn_two' })]),
          { status: 200, headers: { 'content-type': 'application/json' } },
        ),
      );
    await expect(
      listSessions(resolveConfig({ fetch: itemFetch as unknown as typeof globalThis.fetch }), {
        maxItems: 1,
      }),
    ).rejects.toThrow(/1-item boundary/i);

    const loopFetch = vi.fn(() => Promise.resolve(continuing()));
    await expect(
      listSessions(resolveConfig({ fetch: loopFetch as unknown as typeof globalThis.fetch })),
    ).rejects.toThrow(/repeated a continuation/i);
    expect(loopFetch).toHaveBeenCalledTimes(2);
  });

  it('rejects invalid compatibility-walk ceilings before making a request', async () => {
    const fetch = vi.fn();
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    for (const options of [
      { maxPages: 0 },
      { maxPages: 1.5 },
      { maxItems: 0 },
      { maxItems: Number.POSITIVE_INFINITY },
    ]) {
      await expect(listSessions(config, options)).rejects.toBeInstanceOf(ValidationError);
    }
    expect(fetch).not.toHaveBeenCalled();
  });

  it('keeps the timeout active while a successful response body is stalled', async () => {
    vi.useFakeTimers();
    try {
      const fetch = vi.fn().mockResolvedValue(
        new Response(new ReadableStream<Uint8Array>({ start() {} }), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
      );
      const pending = listSessions(
        resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch }),
        { timeoutMs: 25 },
      );
      const assertion = expect(pending).rejects.toBeInstanceOf(TimeoutError);
      await vi.advanceTimersByTimeAsync(25);
      await assertion;
    } finally {
      vi.useRealTimers();
    }
  });

  it('keeps caller cancellation active after response headers arrive', async () => {
    const controller = new AbortController();
    const fetch = vi.fn().mockResolvedValue(
      new Response(new ReadableStream<Uint8Array>({ start() {} }), {
        status: 200,
        headers: { 'content-type': 'application/json' },
      }),
    );
    const pending = listSessions(
      resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch }),
      { signal: controller.signal },
    );
    await vi.waitFor(() => expect(fetch).toHaveBeenCalledTimes(1));
    controller.abort('caller left');
    await expect(pending).rejects.toBeInstanceOf(RequestAborted);
  });

  it('uses one aggregate deadline across every page request', async () => {
    vi.useFakeTimers();
    try {
      let call = 0;
      const fetch = vi.fn(async () => {
        await new Promise<void>((resolve) => setTimeout(resolve, 10));
        call += 1;
        return new Response(JSON.stringify([sessionFixture({ id: `sesn_${String(call)}` })]), {
          status: 200,
          headers: {
            'content-type': 'application/json',
            ...(call === 1 ? { link: '</api/v1/sessions?cursor=second>; rel="next"' } : {}),
          },
        });
      });
      const pending = listSessions(
        resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch }),
        { timeoutMs: 15 },
      );
      const assertion = expect(pending).rejects.toBeInstanceOf(TimeoutError);
      await vi.advanceTimersByTimeAsync(10);
      expect(fetch).toHaveBeenCalledTimes(2);
      await vi.advanceTimersByTimeAsync(5);
      await assertion;
    } finally {
      vi.useRealTimers();
    }
  });

  it('refuses a continuation that could redirect the caller credential', async () => {
    const fetch = vi.fn().mockResolvedValueOnce(
      new Response(JSON.stringify([sessionFixture()]), {
        status: 200,
        headers: {
          'content-type': 'application/json',
          link: '<https://outside.example/api/v1/sessions?cursor=x>; rel="next"',
        },
      }),
    );

    await expect(
      listSessions(resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch })),
    ).rejects.toBeInstanceOf(ProtocolError);
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it('refuses a continuation that leaves the session route', async () => {
    const fetch = vi.fn().mockResolvedValueOnce(
      new Response(JSON.stringify([sessionFixture()]), {
        status: 200,
        headers: {
          'content-type': 'application/json',
          link: '</api/v1/environment-profiles?cursor=x>; rel="next"',
        },
      }),
    );

    await expect(
      listSessions(resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch })),
    ).rejects.toBeInstanceOf(ProtocolError);
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it('still refuses a malformed Link entry that declares a next relation', async () => {
    const fetch = vi.fn().mockResolvedValueOnce(
      new Response(JSON.stringify([sessionFixture()]), {
        status: 200,
        headers: {
          'content-type': 'application/json',
          link: '/api/v1/sessions?cursor=x; rel="next"',
        },
      }),
    );

    await expect(
      listSessions(resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch })),
    ).rejects.toBeInstanceOf(ProtocolError);
    expect(fetch).toHaveBeenCalledTimes(1);
  });

  it('preserves future lifecycle, recovery, and stop-reason vocabulary', () => {
    const future = toSession(
      sessionFixture({
        status: 'hibernating',
        recovery_state: 'self_healing',
        stop_reason: 'new_reason',
      }),
    );
    expect(future).toMatchObject({
      lifecycle: 'unknown',
      reportedStatus: 'hibernating',
      recoveryState: 'self_healing',
      stopReason: 'other',
    });

    for (const reason of STOP_REASONS) {
      expect(toSession(sessionFixture({ stop_reason: reason })).stopReason).toBe(reason);
    }
    expect(toSession(sessionFixture({ stop_reason: null })).stopReason).toBeUndefined();
  });

  it('pins every stop reason and recovery state to the layer2-core enums', () => {
    expect(serializedEnumWords('StopReason')).toEqual(STOP_REASONS);
    expect(serializedEnumWords('RecoveryState')).toEqual(RECOVERY_STATES);

    for (const reason of STOP_REASONS) {
      expect(toSession(sessionFixture({ stop_reason: reason })).stopReason).toBe(reason);
    }
    for (const recoveryState of RECOVERY_STATES) {
      expect(toSession(sessionFixture({ recovery_state: recoveryState })).recoveryState).toBe(
        recoveryState,
      );
    }
  });

  it('keeps all list rows when one lifecycle comes from a newer server', async () => {
    const answer = answering([
      sessionFixture({ id: 'sesn_ready', status: 'ready' }),
      sessionFixture({ id: 'sesn_future', status: 'hibernating' }),
      sessionFixture({ id: 'sesn_idle', status: 'idle' }),
    ]);

    const sessions = await listSessions(resolveConfig({ fetch: answer.fetch }));

    expect(sessions.map(({ id }) => id)).toEqual(['sesn_ready', 'sesn_future', 'sesn_idle']);
    expect(sessions[1]).toMatchObject({
      lifecycle: 'unknown',
      reportedStatus: 'hibernating',
    });
  });

  it('reads an unfamiliar lifecycle under its raw word and keeps known words exact', async () => {
    for (const status of KNOWN_LIFECYCLES) {
      const session = toSession(sessionFixture({ status }));
      expect(session.lifecycle).toBe(status);
      expect(session).not.toHaveProperty('reportedStatus');
    }

    const answer = answering(sessionFixture({ status: 'hibernating' }));
    await expect(
      readSession(resolveConfig({ fetch: answer.fetch }), 'sesn_future'),
    ).resolves.toMatchObject({
      lifecycle: 'unknown',
      reportedStatus: 'hibernating',
    });
  });

  it('keeps optional fields absent and rejects a malformed reported size', () => {
    const sparse = toSession({
      id: 'sesn_sparse',
      status: 'ready',
      kernel: 'future-runtime',
      created_at: 'invalid',
      agent: {
        permission: 'future',
        reasoning_effort: 'future',
        credential: 'future',
        skills: [null, { id: '' }, { id: 'kept' }],
      },
      reach: [null, { id: '' }],
      rotated_at: 'invalid',
      creator: {},
    });
    expect(sparse.reportedStatus).toBeUndefined();
    expect(sparse.agent).toEqual({
      model: undefined,
      permission: undefined,
      reasoningEffort: undefined,
      credential: undefined,
      size: undefined,
      skills: [{ id: 'kept', version: undefined, name: undefined, description: undefined }],
    });
    expect(sparse.reach).toEqual([]);
    expect(sparse.rotatedAt).toBeUndefined();
    expect(sparse.creator).toBeUndefined();

    expect(() =>
      toSession(sessionFixture({ agent: { size: { cpu_millis: 0, memory_mib: 512 } } })),
    ).toThrow(/cpu_millis/i);
    expect(() => toSession(sessionFixture({ agent: { size: 'large' } }))).toThrow(
      /size was not an object/i,
    );
  });

  it('rejects malformed collection and identity boundaries', async () => {
    const malformed = answering({ sessions: 'not-a-list' });
    await expect(listSessions(resolveConfig({ fetch: malformed.fetch }))).rejects.toBeInstanceOf(
      ProtocolError,
    );
    expect(() => toSession(null)).toThrow(ProtocolError);
    expect(() => toSession(sessionFixture({ id: undefined }))).toThrow(ProtocolError);
    expect(() => toSession(sessionFixture({ status: 7 }))).toThrow(ProtocolError);
    expect(() => toSession(sessionFixture({ status: '' }))).toThrow(ProtocolError);
    expect(() => toSession(sessionFixture({ kernel: null }))).toThrow(ProtocolError);
  });
});

describe('the live-session allowance', () => {
  it('reads the caller-scoped advisory response', async () => {
    const { fetch, calls } = answering({ limit: 3, live: 2, exempt: false });
    await expect(readAllowance(resolveConfig({ fetch }))).resolves.toEqual({
      limit: 3,
      live: 2,
      exempt: false,
    });
    expect(calls).toHaveBeenCalledWith(
      '/api/v1/sessions/allowance',
      expect.objectContaining({ method: 'GET' }),
    );
  });

  it('keeps an unconfigured limit absent', async () => {
    const { fetch } = answering({ live: 7, exempt: true });
    await expect(readAllowance(resolveConfig({ fetch }))).resolves.toEqual({
      live: 7,
      exempt: true,
    });
  });

  it('treats malformed advisory fields as permissive defaults', async () => {
    const { fetch } = answering({ limit: 0, live: 'many', exempt: 'yes' });
    await expect(readAllowance(resolveConfig({ fetch }))).resolves.toEqual({
      live: 0,
      exempt: false,
    });
  });
});

describe('capacity pricing', () => {
  const quote = {
    rate_card: 'rate_exact',
    currency: 'USD',
    cpu_millis: 1000,
    memory_mib: 1024,
    micros_per_hour: '45000',
    display: '$0.045',
    per: 'hour',
    quoted_at: '2026-08-23T05:00:00Z',
  };

  it('reads a profile quote and preserves the server display verbatim', async () => {
    const { fetch, calls } = answering(quote);
    await expect(quoteSession(resolveConfig({ fetch }), 'envp_1')).resolves.toEqual({
      priced: true,
      rateCard: 'rate_exact',
      currency: 'USD',
      cpuMillis: 1000,
      memoryMib: 1024,
      microsPerHour: '45000',
      display: '$0.045',
      per: 'hour',
      quotedAt: '2026-08-23T05:00:00Z',
    });
    expect(calls).toHaveBeenCalledWith(
      '/api/v1/pricing/quote?environment_profile=envp_1',
      expect.objectContaining({ method: 'GET' }),
    );
  });

  it('keeps an unpriced quote distinct from a zero quote', async () => {
    const { fetch } = answering({ priced: false });
    await expect(quoteSession(resolveConfig({ fetch }), null)).resolves.toEqual({ priced: false });
  });

  it('sends the exact card the owner confirmed when creating', async () => {
    const { fetch, calls } = answering(
      {
        id: 'sesn_1',
        kernel: 'codex',
        status: 'provisioning',
        created_at: '2026-08-23T05:00:00Z',
        conforms_to: [],
      },
      201,
    );
    await createSession(resolveConfig({ fetch }), {
      cols: 80,
      rows: 24,
      environmentProfile: 'envp_1',
      quotedRateCard: 'rate_exact',
    });

    const [, init] = calls.mock.calls[0] as unknown as [RequestInfo | URL, RequestInit];
    expect(JSON.parse(String(init.body))).toEqual({
      cols: 80,
      rows: 24,
      environment_profile: 'envp_1',
      quoted_rate_card: 'rate_exact',
    });
  });

  it('serializes chat creation and reads the exact worktree projection', async () => {
    const { fetch, calls } = answering(fixture('session.json'), 201);

    await expect(
      createSession(resolveConfig({ fetch }), { surface: 'chat' }),
    ).resolves.toMatchObject({
      chat: {
        worktreeName: 'chat-3',
        worktreePath: '/workspace/chat-3',
        title: 'Fixture chat',
        pausedNotice: 'Paused after inactivity',
      },
    });
    const [, init] = calls.mock.calls[0] as unknown as [RequestInfo | URL, RequestInit];
    expect(JSON.parse(String(init.body))).toEqual({ surface: 'chat' });
  });

  it('refuses malformed required chat worktree facts', () => {
    expect(() =>
      toSession(sessionFixture({ chat: { worktree_name: '', worktree_path: '/w' } })),
    ).toThrow(/worktree name/i);
    expect(() =>
      toSession(sessionFixture({ chat: { worktree_name: 'chat-1', worktree_path: '' } })),
    ).toThrow(/worktree path/i);
  });

  it('reads spend without coercing either monetary integer through a number', async () => {
    const { fetch, calls } = answering({
      priced: true,
      rate_card: 'rate_exact',
      currency: 'USD',
      micros_per_hour: '45000',
      accrued_micros: '90000',
      display: '$0.09',
      billable_seconds: 7200,
      accruing: false,
      unbounded_intervals: 1,
    });
    await expect(readSpend(resolveConfig({ fetch }), 'sesn/one')).resolves.toEqual({
      priced: true,
      rateCard: 'rate_exact',
      currency: 'USD',
      microsPerHour: '45000',
      accruedMicros: '90000',
      display: '$0.09',
      billableSeconds: 7200,
      accruing: false,
      unboundedIntervals: 1,
    });
    expect(calls).toHaveBeenCalledWith(
      '/api/v1/sessions/sesn%2Fone/spend',
      expect.objectContaining({ method: 'GET' }),
    );
  });

  it('refuses a monetary JSON number rather than silently rounding it', async () => {
    const { fetch } = answering({ ...quote, micros_per_hour: 45000 });
    await expect(quoteSession(resolveConfig({ fetch }))).rejects.toThrow('invalid micros_per_hour');
  });
});

describe('session lifecycle and transcript operations', () => {
  it('uses the exact routes, methods, bodies, and binary transcript response', async () => {
    const calls: { readonly url: string; readonly init: RequestInit }[] = [];
    const fetch = vi.fn((url: string | URL | Request, init?: RequestInit) => {
      const request = { url: String(url), init: init ?? {} };
      calls.push(request);
      if (request.url.endsWith('/transcript')) {
        return Promise.resolve(
          new Response(new Uint8Array([0x1b, 0x5b, 0x32, 0x4a]), {
            status: 200,
            headers: { 'Content-Type': 'application/octet-stream' },
          }),
        );
      }
      if (request.url.endsWith('/plane') || request.url.endsWith('/revive')) {
        return Promise.resolve(
          new Response(JSON.stringify(sessionFixture({ status: 'rotating' })), {
            status: 200,
            headers: { 'Content-Type': 'application/json' },
          }),
        );
      }
      return Promise.resolve(new Response(null, { status: 204 }));
    });
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    await expect(changePlane(config, 'sesn/one', 'conversation')).resolves.toMatchObject({
      lifecycle: 'rotating',
    });
    await terminateSession(config, 'sesn/one');
    await stopSession(config, 'sesn/one');
    await expect(reviveSession(config, 'sesn/one')).resolves.toMatchObject({
      lifecycle: 'rotating',
    });
    await deleteSession(config, 'sesn/one');
    await expect(fetchTranscript(config, 'sesn/one')).resolves.toEqual(
      new Uint8Array([0x1b, 0x5b, 0x32, 0x4a]),
    );

    expect(calls.map(({ url, init }) => [url, init.method])).toEqual([
      ['/api/v1/sessions/sesn%2Fone/plane', 'POST'],
      ['/api/v1/sessions/sesn%2Fone/terminate', 'POST'],
      ['/api/v1/sessions/sesn%2Fone/stop', 'POST'],
      ['/api/v1/sessions/sesn%2Fone/revive', 'POST'],
      ['/api/v1/sessions/sesn%2Fone', 'DELETE'],
      ['/api/v1/sessions/sesn%2Fone/transcript', 'GET'],
    ]);
    expect(JSON.parse(String(calls[0]?.init.body))).toEqual({ plane: 'conversation' });
    expect(new Headers(calls[5]?.init.headers).get('accept')).toBe('application/octet-stream');
  });

  it('rejects a plane and incomplete creation geometry before fetching', async () => {
    const fetch = vi.fn();
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    await expect(changePlane(config, 's', 'future' as 'terminal')).rejects.toBeInstanceOf(
      ValidationError,
    );
    await expect(createSession(config, { cols: 80 })).rejects.toBeInstanceOf(ValidationError);
    await expect(createSession(config, { cols: 0, rows: 24 })).rejects.toBeInstanceOf(
      ValidationError,
    );
    expect(fetch).not.toHaveBeenCalled();
  });

  it('decodes a transcript problem document through the shared error taxonomy', async () => {
    const fetch = vi.fn(() =>
      Promise.resolve(
        new Response(
          JSON.stringify({ status: 404, detail: 'session not found', code: 'not_found' }),
          { status: 404, headers: { 'Content-Type': 'application/problem+json' } },
        ),
      ),
    );
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });

    const failure = await fetchTranscript(config, 'missing').catch((cause: unknown) => cause);
    expect(failure).toBeInstanceOf(ApiError);
    expect((failure as ApiError).code).toBe('not_found');
  });

  it('propagates pre-abort through every session operation', async () => {
    const fetch = vi.fn();
    const config = resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch });
    const controller = new AbortController();
    controller.abort('cancelled');
    const options = { signal: controller.signal };
    const operations = [
      () => listSessions(config, options),
      () => readAllowance(config, options),
      () => readSession(config, 's', options),
      () => listKernels(config, options),
      () => createSession(config, {}, options),
      () => quoteSession(config, undefined, options),
      () => readSpend(config, 's', options),
      () => changePlane(config, 's', 'terminal', options),
      () => terminateSession(config, 's', options),
      () => stopSession(config, 's', options),
      () => reviveSession(config, 's', options),
      () => deleteSession(config, 's', options),
      () => fetchTranscript(config, 's', options),
    ];

    for (const operation of operations) {
      await expect(operation()).rejects.toBeInstanceOf(RequestAborted);
    }
    expect(fetch).not.toHaveBeenCalled();
  });
});

describe('pricing boundary validation', () => {
  it('keeps an unpriced spend distinct from a zero-priced spend', () => {
    expect(toSessionSpend({ priced: false, unbounded_intervals: 2 })).toEqual({
      priced: false,
      accruing: false,
      unboundedIntervals: 2,
    });
  });

  it.each([
    [null, /pricing quote was not an object/i],
    [{ priced: true, per: 'minute' }, /unrecognised interval/i],
    [
      {
        priced: true,
        per: 'hour',
        rate_card: '',
        currency: 'USD',
        cpu_millis: 1,
        memory_mib: 1,
        micros_per_hour: '1',
        display: '$0',
        quoted_at: 'now',
      },
      /rate_card/i,
    ],
    [
      {
        priced: true,
        per: 'hour',
        rate_card: 'r',
        currency: 'USD',
        cpu_millis: -1,
        memory_mib: 1,
        micros_per_hour: '1',
        display: '$0',
        quoted_at: 'now',
      },
      /cpu_millis/i,
    ],
    [
      {
        priced: true,
        per: 'hour',
        rate_card: 'r',
        currency: 'USD',
        cpu_millis: 1,
        memory_mib: 1,
        micros_per_hour: '01',
        display: '$0',
        quoted_at: 'now',
      },
      /micros_per_hour/i,
    ],
  ])('rejects a malformed quote %#', (body, expected) => {
    expect(() => toPricingQuote(body)).toThrow(expected);
  });

  it.each([
    [null, /session spend was not an object/i],
    [{ priced: false, unbounded_intervals: -1 }, /unbounded_intervals/i],
    [{ priced: true, unbounded_intervals: 0, accruing: 'yes' }, /pricing state/i],
  ])('rejects a malformed spend %#', (body, expected) => {
    expect(() => toSessionSpend(body)).toThrow(expected);
  });
});
