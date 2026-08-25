import { describe, expect, it, vi } from 'vitest';
import {
  headersFor,
  resolveConfig,
  responseRequestId,
  urlOf,
  type ResponseMetadata,
} from '../src/config.js';
import { request } from '../src/http.js';
import {
  ApiError,
  CapabilityRefused,
  CredentialError,
  DeploymentAtCapacity,
  ProtocolError,
  RequestAborted,
  RequestIdError,
  ResponseObserverError,
  SessionLimitReached,
  TimeoutError,
  TransportError,
  ValidationError,
} from '../src/errors.js';

/**
 * The SDK's transport.
 *
 * No DOM. This is what makes the client usable from somewhere that is not the
 * dashboard, so it is tested the way somebody else would reach it: an injected
 * `fetch`, a configured host, a token that is read per request.
 *
 * @vitest-environment node
 */

/** A `fetch` that records what it was asked and answers what a test chose. */
function recorder(
  answer: Partial<Response> & { json?: () => Promise<unknown> },
  headers: HeadersInit = {},
) {
  const calls: { url: string; init: RequestInit }[] = [];
  const impl = vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(url), init: init ?? {} });
    const status = answer.status ?? (answer.ok === false ? 500 : 200);
    let body: string | null = null;
    if (status !== 204) {
      try {
        body = JSON.stringify(answer.json === undefined ? {} : await answer.json());
      } catch {
        body = 'not json';
      }
    }
    const responseHeaders = new Headers({ 'content-type': 'application/json' });
    for (const [name, value] of new Headers(headers)) {
      responseHeaders.set(name, value);
    }
    return new Response(body, {
      status,
      headers: responseHeaders,
    });
  });
  return { calls, impl: impl as unknown as typeof globalThis.fetch };
}

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

const nextTurn = () => new Promise<void>((resolve) => setTimeout(resolve, 0));

describe('where the client points', () => {
  it('defaults to same-origin, which is what the dashboard needs', () => {
    // The bundle is served by a proxy that forwards /api/ onward, so a host
    // compiled in here would have to be rebuilt to move the service.
    expect(urlOf(resolveConfig(), '/api/v1/sessions')).toBe('/api/v1/sessions');
  });

  it('points at a configured deployment', () => {
    const config = resolveConfig({ baseUrl: 'https://bot-api.example' });
    expect(urlOf(config, '/api/v1/sessions')).toBe('https://bot-api.example/api/v1/sessions');
  });

  it('strips a trailing slash rather than tolerating it', () => {
    // `https://host/` joined to `/api/v1` is `https://host//api/v1`, which
    // some gateways route and others reject — a difference that only shows up
    // in production.
    const config = resolveConfig({ baseUrl: 'https://bot-api.example/' });
    expect(urlOf(config, '/api/v1/sessions')).toBe('https://bot-api.example/api/v1/sessions');
  });

  it('says so when there is no fetch anywhere to use', async () => {
    // Better than a bare TypeError from inside the request, which reads as a
    // bug in the SDK rather than as a missing argument.
    const real = globalThis.fetch;
    try {
      (globalThis as { fetch?: unknown }).fetch = undefined;
      await expect(request(resolveConfig(), '/x')).rejects.toThrow(
        'inject one through ClientConfig.fetch',
      );
    } finally {
      globalThis.fetch = real;
    }
  });

  it('reads the platform fetch when it is called, not when it is configured', async () => {
    // A client is usually built at module scope. Captured at construction, a
    // `fetch` installed afterwards — a retry wrapper, a tracing shim, a test
    // stub — was silently ignored for the life of the process.
    const config = resolveConfig();
    const real = globalThis.fetch;
    try {
      const later = vi.fn(() =>
        Promise.resolve(
          new Response(JSON.stringify({ ok: 1 }), {
            status: 200,
            headers: { 'content-type': 'application/json' },
          }),
        ),
      );
      globalThis.fetch = later as unknown as typeof fetch;
      await expect(request(config, '/x')).resolves.toEqual({ ok: 1 });
      expect(later).toHaveBeenCalled();
    } finally {
      globalThis.fetch = real;
    }
  });
});

describe('the credential', () => {
  it('sends none by default, because the dashboard uses a cookie', async () => {
    // The one credential form a browser cannot leak to script. Handling a
    // token there would give that up. Read lowercased throughout: `Headers`
    // normalises, and header names are case-insensitive on the wire.
    const headers = await headersFor(resolveConfig());
    expect(headers.authorization).toBeUndefined();
  });

  it('presents a bearer token when one is configured', async () => {
    const headers = await headersFor(resolveConfig({ credential: () => 'tok_abc' }));
    expect(headers.authorization).toBe('Bearer tok_abc');
  });

  it('resolves the token per request, so an expiring one can be replaced', async () => {
    // Captured once, a client works until the token expires and then fails
    // permanently with no way to recover but a reload.
    let current = 'first';
    const config = resolveConfig({ credential: () => current });
    expect((await headersFor(config)).authorization).toBe('Bearer first');
    current = 'second';
    expect((await headersFor(config)).authorization).toBe('Bearer second');
  });

  it('awaits a credential that has to be fetched', async () => {
    const config = resolveConfig({ credential: () => Promise.resolve('tok_async') });
    expect((await headersFor(config)).authorization).toBe('Bearer tok_async');
  });

  it('reports an empty bearer as a credential failure', async () => {
    // `Bearer ` with nothing after it is refused with a 401 identical to an
    // expired token's, which sends the reader to the wrong problem.
    const config = resolveConfig({ credential: () => '' });
    await expect(headersFor(config)).rejects.toBeInstanceOf(CredentialError);
  });

  it('lets a caller add a header without losing the rest', async () => {
    // Names come back lowercased: everything is normalised through `Headers`,
    // and header names are case-insensitive on the wire.
    const headers = await headersFor(resolveConfig(), { 'X-Trace': 'abc' });
    expect(headers['x-trace']).toBe('abc');
    expect(headers['content-type']).toBe('application/json');
  });

  it('lets a caller replace a default rather than sending both', async () => {
    // Built by hand, a caller's `content-type` was added *beside* the default
    // `Content-Type` and the request carried two, leaving the service to pick.
    const headers = await headersFor(resolveConfig(), { 'content-type': 'text/plain' });
    expect(headers['content-type']).toBe('text/plain');
    expect(Object.keys(headers).filter((k) => k.toLowerCase() === 'content-type')).toHaveLength(1);
  });
});

describe('request and response correlation', () => {
  it('sends a caller override exactly and does not consult the default provider', async () => {
    const provider = vi.fn(() => 'provider-value');
    const { calls, impl } = recorder({});
    await request(resolveConfig({ fetch: impl, requestIdProvider: provider }), '/api/v1/sessions', {
      requestId: 'Support.Case/488:Exact',
    });

    expect(provider).not.toHaveBeenCalled();
    expect(new Headers(calls[0]?.init.headers).get('x-request-id')).toBe('Support.Case/488:Exact');
  });

  it('rejects unusable caller and provider values before fetch', async () => {
    for (const requestId of ['', ' padded ', 'line\nbreak', 'opaque-\u00ff']) {
      const fetch = vi.fn();
      await expect(
        request(resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch }), '/x', {
          requestId,
        }),
      ).rejects.toBeInstanceOf(ValidationError);
      expect(fetch).not.toHaveBeenCalled();
    }

    for (const requestIdProvider of [
      () => ' \t ',
      () => 'opaque-\u00ff',
      () => {
        throw new Error('trace context unavailable');
      },
    ]) {
      const fetch = vi.fn();
      await expect(
        request(
          resolveConfig({
            fetch: fetch as unknown as typeof globalThis.fetch,
            requestIdProvider,
          }),
          '/x',
        ),
      ).rejects.toBeInstanceOf(RequestIdError);
      expect(fetch).not.toHaveBeenCalled();
    }
  });

  it('observes success metadata before attempting to decode the body', async () => {
    let phase = 'received';
    const encoded = new TextEncoder().encode('{not json');
    const read = vi
      .fn()
      .mockImplementationOnce(() => {
        expect(phase).toBe('observed');
        return Promise.resolve({ done: false, value: encoded });
      })
      .mockResolvedValueOnce({ done: true, value: undefined });
    const getReader = vi.fn(() => ({
      cancel: () => Promise.resolve(),
      read,
      releaseLock: () => undefined,
    }));
    const response = {
      body: { getReader },
      headers: new Headers({ 'x-request-id': 'response-before-decode' }),
      ok: true,
      status: 200,
    } as unknown as Response;
    const fetch = vi.fn(() => Promise.resolve(response));
    const observed: ResponseMetadata[] = [];
    const config = resolveConfig({
      fetch: fetch as unknown as typeof globalThis.fetch,
      responseObserver: (metadata) => {
        expect(phase).toBe('received');
        observed.push(metadata);
        phase = 'observed';
      },
    });

    await expect(request(config, '/x')).rejects.toBeInstanceOf(ProtocolError);
    expect(observed).toEqual([{ status: 200, requestId: 'response-before-decode' }]);
    expect(getReader).toHaveBeenCalledOnce();
  });

  it('reports observer failure explicitly without consuming the body', async () => {
    const getReader = vi.fn();
    const response = {
      body: { getReader },
      headers: new Headers({ 'x-request-id': 'response-observer-failure' }),
      ok: true,
      status: 200,
    } as unknown as Response;
    const fetch = vi.fn(() => Promise.resolve(response));
    const config = resolveConfig({
      fetch: fetch as unknown as typeof globalThis.fetch,
      responseObserver: () => {
        throw new Error('metadata sink unavailable');
      },
    });

    const failure = await request(config, '/x').catch((cause: unknown) => cause);
    expect(failure).toBeInstanceOf(ResponseObserverError);
    expect((failure as Error).cause).toBeInstanceOf(Error);
    expect(getReader).not.toHaveBeenCalled();
  });

  it('does not continue to credentials or fetch after a request-id deadline', async () => {
    const requestId = deferred<string>();
    const requestIdProvider = vi.fn(() => requestId.promise);
    const credential = vi.fn(() => 'tok_late');
    const fetch = vi.fn();
    const config = resolveConfig({
      credential,
      fetch: fetch as unknown as typeof globalThis.fetch,
      requestIdProvider,
    });

    await expect(request(config, '/x', { timeoutMs: 5 })).rejects.toBeInstanceOf(TimeoutError);
    expect(requestIdProvider).toHaveBeenCalledOnce();

    requestId.resolve('caller-after-timeout');
    await nextTurn();

    expect(credential).not.toHaveBeenCalled();
    expect(fetch).not.toHaveBeenCalled();
  });

  it('does not observe a fetch response that arrives after the deadline', async () => {
    const fetched = deferred<Response>();
    const responseObserver = vi.fn();
    const config = resolveConfig({
      fetch: vi.fn(() => fetched.promise) as unknown as typeof globalThis.fetch,
      responseObserver,
    });

    await expect(request(config, '/x', { timeoutMs: 5 })).rejects.toBeInstanceOf(TimeoutError);

    fetched.resolve(new Response('{}', { status: 200 }));
    await nextTurn();

    expect(responseObserver).not.toHaveBeenCalled();
  });

  it('does not consume a response after cancellation wins over its observer', async () => {
    const observer = deferred<void>();
    const observerStarted = deferred<void>();
    const responseObserver = vi.fn(() => {
      observerStarted.resolve(undefined);
      return observer.promise;
    });
    const getReader = vi.fn();
    const response = {
      body: { getReader },
      headers: new Headers(),
      ok: true,
      status: 200,
    } as unknown as Response;
    const fetch = vi.fn(() => Promise.resolve(response));
    const controller = new AbortController();
    const config = resolveConfig({
      fetch: fetch as unknown as typeof globalThis.fetch,
      responseObserver,
    });
    const pending = request(config, '/x', { signal: controller.signal });
    await observerStarted.promise;

    controller.abort('cancelled while observing');

    await expect(pending).rejects.toBeInstanceOf(RequestAborted);
    observer.resolve(undefined);
    await nextTurn();

    expect(responseObserver).toHaveBeenCalledOnce();
    expect(getReader).not.toHaveBeenCalled();
  });

  it('treats missing, blank, malformed, and non-visible response ids as absent', () => {
    const responseWith = (value: string | null): Response =>
      ({ headers: { get: () => value } }) as unknown as Response;
    expect(responseRequestId(responseWith('response-visible/488'))).toBe('response-visible/488');
    for (const value of [null, '', ' \t ', 'line\nbreak', 'opaque-\u00ff']) {
      expect(responseRequestId(responseWith(value))).toBeUndefined();
    }
    expect(
      responseRequestId({
        headers: {
          get: () => {
            throw new Error('bad headers');
          },
        },
      } as unknown as Response),
    ).toBeUndefined();
  });
});

describe('cookies', () => {
  it('are included by default', async () => {
    const { calls, impl } = recorder({});
    await request(resolveConfig({ fetch: impl }), '/api/v1/sessions');
    expect(calls[0]?.init.credentials).toBe('include');
  });

  it('are left out when a cross-origin caller turns them off', async () => {
    // Sending them to another origin either fails preflight or quietly
    // authenticates as somebody the caller did not intend.
    const { calls, impl } = recorder({});
    await request(resolveConfig({ fetch: impl, cookies: false }), '/api/v1/sessions');
    expect(calls[0]?.init.credentials).toBe('omit');
  });
});

describe('what comes back', () => {
  it('parses a body', async () => {
    const { impl } = recorder({ json: () => Promise.resolve({ sessions: [] }) });
    await expect(request(resolveConfig({ fetch: impl }), '/api/v1/sessions')).resolves.toEqual({
      sessions: [],
    });
  });

  it('reads a 204 as an answer rather than an empty body', async () => {
    // Terminate and delete answer this way. Awaiting a body would hang on the
    // parse of nothing.
    const { impl } = recorder({ status: 204, json: () => Promise.reject(new Error('no body')) });
    await expect(request(resolveConfig({ fetch: impl }), '/x')).resolves.toBeNull();
  });

  it('carries the service’s own detail rather than the status line', async () => {
    const { impl } = recorder({
      ok: false,
      status: 409,
      json: () => Promise.resolve({ detail: 'this session is already running' }),
    });
    await expect(request(resolveConfig({ fetch: impl }), '/x')).rejects.toThrow(
      'this session is already running',
    );
  });

  it('exposes a generic structured refusal response id and tolerates its absence', async () => {
    const problem = {
      status: 409,
      detail: 'this session is already running',
      code: 'turn_in_progress',
    };
    const correlated = recorder(
      { ok: false, status: 409, json: () => Promise.resolve(problem) },
      { 'x-request-id': 'response-api-488' },
    );
    const withId = await request(resolveConfig({ fetch: correlated.impl }), '/x').catch(
      (cause: unknown) => cause,
    );
    expect(withId).toBeInstanceOf(ApiError);
    expect((withId as ApiError).requestId).toBe('response-api-488');

    const uncorrelated = recorder({ ok: false, status: 409, json: () => Promise.resolve(problem) });
    const withoutId = await request(resolveConfig({ fetch: uncorrelated.impl }), '/x').catch(
      (cause: unknown) => cause,
    );
    expect(withoutId).toBeInstanceOf(ApiError);
    expect((withoutId as ApiError).requestId).toBeUndefined();
  });

  it('falls back to the status when the body is not a problem document', async () => {
    const { impl } = recorder({
      ok: false,
      status: 502,
      json: () => Promise.reject(new Error('not json')),
    });
    await expect(request(resolveConfig({ fetch: impl }), '/x')).rejects.toThrow('502');
  });

  it('names the ability when the service refused one', async () => {
    // §3: a refusal that does not name what is missing has thrown away the
    // only part a caller could act on. Keyed on the code the service actually
    // sends — checked against `layer1-public`'s problem document rather than
    // guessed, after the first version of this read a field REST does not
    // send and so could never have fired.
    const { impl } = recorder(
      {
        ok: false,
        status: 422,
        json: () =>
          Promise.resolve({
            detail: 'capability not supported: conversation',
            code: 'capability_unsupported',
            capability: 'conversation',
          }),
      },
      { 'x-request-id': 'response-capability-488' },
    );
    const failure = await request(resolveConfig({ fetch: impl }), '/x').catch(
      (cause: unknown) => cause,
    );
    expect(failure).toBeInstanceOf(CapabilityRefused);
    expect((failure as CapabilityRefused).capability).toBe('conversation');
    expect((failure as CapabilityRefused).requestId).toBe('response-capability-488');
  });

  it('does not read a malformed request as a refusal', async () => {
    // Both are 422. One is worth fixing and resending; the other never
    // succeeds however many times it is sent, which is why the code decides
    // and the status does not.
    const { impl } = recorder({
      ok: false,
      status: 422,
      json: () => Promise.resolve({ detail: 'cols must be positive', code: 'invalid_request' }),
    });
    const failure = await request(resolveConfig({ fetch: impl }), '/x').catch(
      (cause: unknown) => cause,
    );
    expect(failure).toBeInstanceOf(ApiError);
    expect(failure).not.toBeInstanceOf(CapabilityRefused);
  });

  it('reports a capability problem that omitted its typed field as malformed', async () => {
    const { impl } = recorder({
      ok: false,
      status: 422,
      json: () => Promise.resolve({ detail: 'not supported', code: 'capability_unsupported' }),
    });
    const failure = await request(resolveConfig({ fetch: impl }), '/x').catch(
      (cause: unknown) => cause,
    );
    expect(failure).toBeInstanceOf(ProtocolError);
  });

  it('carries a live-session refusal and both of its numbers', async () => {
    const { impl } = recorder(
      {
        ok: false,
        status: 409,
        json: () =>
          Promise.resolve({
            detail: '3 of 3 sessions are already running for this owner',
            code: 'session_limit_reached',
            limit: 3,
            live: 3,
          }),
      },
      { 'x-request-id': 'response-limit-488' },
    );
    const failure = await request(resolveConfig({ fetch: impl }), '/x').catch(
      (cause: unknown) => cause,
    );
    expect(failure).toBeInstanceOf(SessionLimitReached);
    expect((failure as SessionLimitReached).limit).toBe(3);
    expect((failure as SessionLimitReached).live).toBe(3);
    expect((failure as SessionLimitReached).requestId).toBe('response-limit-488');
  });

  it('carries a deployment-capacity refusal and all four of its figures', async () => {
    const { impl } = recorder(
      {
        ok: false,
        status: 503,
        json: () =>
          Promise.resolve({
            detail: 'this deployment has no room for that session',
            code: 'deployment_at_capacity',
            requested_cpu_millis: 2_000,
            requested_memory_mib: 4_096,
            free_cpu_millis: 750,
            free_memory_mib: 1_536,
          }),
      },
      { 'x-request-id': 'response-capacity-488' },
    );
    const failure = await request(resolveConfig({ fetch: impl }), '/x').catch(
      (cause: unknown) => cause,
    );

    expect(failure).toBeInstanceOf(DeploymentAtCapacity);
    expect(failure).toMatchObject({
      requestedCpuMillis: 2_000,
      requestedMemoryMib: 4_096,
      freeCpuMillis: 750,
      freeMemoryMib: 1_536,
      isRetryable: true,
      requestId: 'response-capacity-488',
    });
  });

  it('does not infer deployment capacity from an unrelated 503', async () => {
    const { impl } = recorder({
      ok: false,
      status: 503,
      json: () => Promise.resolve({ detail: 'runtime unavailable', code: 'runtime_unavailable' }),
    });
    const failure = await request(resolveConfig({ fetch: impl }), '/x').catch(
      (cause: unknown) => cause,
    );

    expect(failure).toBeInstanceOf(ApiError);
    expect(failure).not.toBeInstanceOf(DeploymentAtCapacity);
  });

  it('uses the problem code rather than treating every 409 as a session limit', async () => {
    const { impl } = recorder({
      ok: false,
      status: 409,
      json: () => Promise.resolve({ detail: 'this session has ended', code: 'session_terminated' }),
    });
    const failure = await request(resolveConfig({ fetch: impl }), '/x').catch(
      (cause: unknown) => cause,
    );
    expect(failure).toBeInstanceOf(ApiError);
    expect(failure).not.toBeInstanceOf(SessionLimitReached);
    expect((failure as ApiError).code).toBe('session_terminated');
  });

  it('keeps malformed session-limit numbers absent', async () => {
    const { impl } = recorder({
      ok: false,
      status: 409,
      json: () =>
        Promise.resolve({
          detail: 'limit reached',
          code: 'session_limit_reached',
          limit: 'three',
          live: -1,
        }),
    });
    const failure = await request(resolveConfig({ fetch: impl }), '/x').catch(
      (cause: unknown) => cause,
    );
    expect(failure).toBeInstanceOf(SessionLimitReached);
    expect((failure as SessionLimitReached).limit).toBeUndefined();
    expect((failure as SessionLimitReached).live).toBeUndefined();
  });

  it('reports an unreachable service as a transport failure', async () => {
    const impl = vi.fn(() => Promise.reject(new Error('offline')));
    const failure = await request(
      resolveConfig({ fetch: impl as unknown as typeof fetch }),
      '/x',
    ).catch((cause: unknown) => cause);
    expect(failure).toBeInstanceOf(TransportError);
    expect((failure as TransportError).isRetryable).toBe(true);
  });
});

describe('telling a refusal from a failure', () => {
  it('marks only 401 as not-signed-in', () => {
    expect(new ApiError(401, 'nope').isUnauthenticated).toBe(true);
    expect(new ApiError(403, 'not yours').isUnauthenticated).toBe(false);
    expect(new ApiError(500, 'boom').isUnauthenticated).toBe(false);
  });

  it('marks what is worth retrying', () => {
    // A caller that retried a 400 would loop forever on a request that can
    // never succeed.
    expect(new ApiError(500, '').isRetryable).toBe(true);
    expect(new ApiError(429, '').isRetryable).toBe(true);
    expect(new ApiError(400, '').isRetryable).toBe(false);
    expect(new ApiError(404, '').isRetryable).toBe(false);
  });

  it('keeps a refusal recognisable as both', () => {
    // It is an ApiError so existing handling still catches it, and a
    // CapabilityRefused so a surface can say which ability is missing.
    const refused = new CapabilityRefused(400, 'no', 'terminal');
    expect(refused).toBeInstanceOf(ApiError);
    expect(refused.isRetryable).toBe(false);
  });
});
