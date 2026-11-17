// @vitest-environment node
import { describe, expect, it, vi } from 'vitest';
import {
  headersFor,
  resolveConfig,
  responseRequestId,
  urlOf,
  type ResponseMetadata,
} from '../config.js';
import {
  errorFromResponse,
  raceWithAbortSignal,
  request,
  requestBytes,
  requestResponse,
  requestVoid,
  throwIfSignalAborted,
  withOperationDeadline,
} from '../http.js';
import {
  ApiError,
  CredentialError,
  ProtocolError,
  RequestAborted,
  RequestIdError,
  ResponseObserverError,
  TimeoutError,
  TransportError,
  ValidationError,
} from '../errors.js';

/** @vitest-environment node */

function response(body: BodyInit | null = '{}', init: ResponseInit = {}): Response {
  return new Response(body, {
    status: 200,
    headers: { 'content-type': 'application/json' },
    ...init,
  });
}

function recorder(answer: Response = response()) {
  const calls: { readonly url: string; readonly init: RequestInit }[] = [];
  const fetch = vi.fn((input: string | URL | Request, init?: RequestInit) => {
    calls.push({ url: String(input), init: init ?? {} });
    return Promise.resolve(answer);
  });
  return { calls, fetch: fetch as unknown as typeof globalThis.fetch };
}

describe('transport configuration', () => {
  it('normalises deployment roots and same-origin paths', () => {
    expect(urlOf(resolveConfig(), '/api/v1/conformance')).toBe('/api/v1/conformance');
    expect(urlOf(resolveConfig({ baseUrl: '/gateway/' }), '/ready')).toBe('/gateway/ready');
    expect(urlOf(resolveConfig({ baseUrl: 'https://cma.example/' }), '/ready')).toBe(
      'https://cma.example/ready',
    );
    expect(() => urlOf(resolveConfig(), 'ready')).toThrow(ValidationError);
  });

  it.each([
    'not a URL',
    'ftp://cma.example',
    'https://user@cma.example',
    'https://cma.example?mode=test',
    'https://cma.example/#fragment',
  ])('rejects an unsafe deployment root: %s', (baseUrl) => {
    expect(() => resolveConfig({ baseUrl })).toThrow(ValidationError);
  });

  it('validates default timeouts', () => {
    for (const requestTimeoutMs of [0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1]) {
      expect(() => resolveConfig({ requestTimeoutMs })).toThrow(ValidationError);
    }
  });

  it('uses the platform fetch at call time', async () => {
    const configured = resolveConfig();
    const original = globalThis.fetch;
    const later = vi.fn(() => Promise.resolve(response('{"ok":true}')));
    try {
      globalThis.fetch = later as unknown as typeof globalThis.fetch;
      await expect(request(configured, '/ready')).resolves.toEqual({ ok: true });
    } finally {
      globalThis.fetch = original;
    }
  });

  it('reports an unavailable platform fetch', async () => {
    const configured = resolveConfig();
    const original = globalThis.fetch;
    try {
      (globalThis as { fetch?: unknown }).fetch = undefined;
      await expect(request(configured, '/ready')).rejects.toBeInstanceOf(TransportError);
    } finally {
      globalThis.fetch = original;
    }
  });
});

describe('credentials and correlation', () => {
  it('resolves rotating credentials and merges caller headers', async () => {
    let token = 'first';
    const configured = resolveConfig({ credential: () => token });
    expect(await headersFor(configured, { Accept: 'application/json' })).toMatchObject({
      accept: 'application/json',
      authorization: 'Bearer first',
      'content-type': 'application/json',
    });
    token = 'second';
    expect((await headersFor(configured)).authorization).toBe('Bearer second');
    expect((await headersFor(configured, undefined, { authenticate: false })).authorization).toBe(
      undefined,
    );
  });

  it('wraps failed and invalid credentials', async () => {
    await expect(headersFor(resolveConfig({ credential: () => '' }))).rejects.toBeInstanceOf(
      CredentialError,
    );
    await expect(
      headersFor(
        resolveConfig({
          credential: () => {
            throw new Error('vault unavailable');
          },
        }),
      ),
    ).rejects.toBeInstanceOf(CredentialError);
  });

  it('sends one request id and exposes response metadata', async () => {
    const observed: ResponseMetadata[] = [];
    const transport = recorder(
      response('{"ok":true}', { headers: { 'x-request-id': 'response-801' } }),
    );
    const configured = resolveConfig({
      fetch: transport.fetch,
      requestIdProvider: () => 'generated-801',
      responseObserver: (metadata) => {
        observed.push(metadata);
      },
    });

    await request(configured, '/ready', { requestId: 'caller-801' });
    expect(new Headers(transport.calls[0]?.init.headers).get('x-request-id')).toBe('caller-801');
    expect(observed).toEqual([{ status: 200, requestId: 'response-801' }]);
  });

  it('rejects unusable request ids before transport', async () => {
    const fetch = vi.fn();
    await expect(
      request(resolveConfig({ fetch: fetch as unknown as typeof globalThis.fetch }), '/ready', {
        requestId: 'line\nbreak',
      }),
    ).rejects.toBeInstanceOf(ValidationError);
    await expect(
      request(
        resolveConfig({
          fetch: fetch as unknown as typeof globalThis.fetch,
          requestIdProvider: () => {
            throw new Error('generator failed');
          },
        }),
        '/ready',
      ),
    ).rejects.toBeInstanceOf(RequestIdError);
    expect(fetch).not.toHaveBeenCalled();
  });

  it('treats unusable response ids as absent', () => {
    const withHeader = (value: string | null) =>
      ({ headers: { get: () => value } }) as unknown as Response;
    expect(responseRequestId(withHeader('visible/801'))).toBe('visible/801');
    for (const value of [null, '', '  ', 'line\nbreak', 'non-ascii-\u00ff']) {
      expect(responseRequestId(withHeader(value))).toBeUndefined();
    }
    expect(
      responseRequestId({
        headers: {
          get: () => {
            throw new Error('malformed headers');
          },
        },
      } as unknown as Response),
    ).toBeUndefined();
  });

  it('stops before body decoding when the observer fails', async () => {
    const configured = resolveConfig({
      fetch: recorder(response('{bad json')).fetch,
      responseObserver: () => {
        throw new Error('sink unavailable');
      },
    });
    await expect(request(configured, '/ready')).rejects.toBeInstanceOf(ResponseObserverError);
  });
});

describe('request boundaries', () => {
  it('encodes JSON and applies credentials, status, and headers', async () => {
    const transport = recorder(response('{"accepted":true}', { status: 202 }));
    const configured = resolveConfig({
      cookies: false,
      credential: () => Promise.resolve('token-801'),
      fetch: transport.fetch,
    });
    await expect(
      request(configured, '/api/v1/actions', {
        method: 'POST',
        body: { action: 'inspect' },
        expectedStatus: [200, 202],
        headers: { 'x-purpose': 'test' },
      }),
    ).resolves.toEqual({ accepted: true });

    expect(transport.calls[0]).toMatchObject({
      url: '/api/v1/actions',
      init: { method: 'POST', credentials: 'omit', body: '{"action":"inspect"}' },
    });
    const headers = new Headers(transport.calls[0]?.init.headers);
    expect(headers.get('authorization')).toBe('Bearer token-801');
    expect(headers.get('x-purpose')).toBe('test');
  });

  it('handles empty success responses and rejects malformed JSON', async () => {
    await expect(
      request(resolveConfig({ fetch: recorder(response(null, { status: 204 })).fetch }), '/ready'),
    ).resolves.toBeNull();
    await expect(
      request(resolveConfig({ fetch: recorder(response('  ')).fetch }), '/ready'),
    ).resolves.toBeNull();
    await expect(
      request(resolveConfig({ fetch: recorder(response('{bad')).fetch }), '/ready'),
    ).rejects.toBeInstanceOf(ProtocolError);
  });

  it('supports response, void, and byte-oriented helpers', async () => {
    const raw = response('body', { status: 202 });
    await expect(
      requestResponse(resolveConfig({ fetch: recorder(raw).fetch }), '/ready', {
        expectedStatus: 202,
      }),
    ).resolves.toBe(raw);
    await expect(
      requestVoid(resolveConfig({ fetch: recorder(response(null, { status: 204 })).fetch }), '/x', {
        expectedStatus: 204,
      }),
    ).resolves.toBeUndefined();
    await expect(
      requestVoid(resolveConfig({ fetch: recorder(response('unexpected')).fetch }), '/x'),
    ).rejects.toBeInstanceOf(ProtocolError);
    await expect(
      requestBytes(resolveConfig({ fetch: recorder(response('abc')).fetch }), '/x'),
    ).resolves.toEqual(new TextEncoder().encode('abc'));
  });

  it('rejects unexpected success statuses and unencodable request bodies', async () => {
    await expect(
      request(resolveConfig({ fetch: recorder(response('{}', { status: 201 })).fetch }), '/x', {
        expectedStatus: 200,
      }),
    ).rejects.toBeInstanceOf(ProtocolError);

    const circular: { self?: unknown } = {};
    circular.self = circular;
    await expect(
      request(resolveConfig({ fetch: recorder().fetch }), '/x', { body: circular }),
    ).rejects.toBeInstanceOf(ValidationError);
    await expect(
      request(resolveConfig({ fetch: recorder().fetch }), '/x', { body: Symbol('no-json') }),
    ).rejects.toBeInstanceOf(ValidationError);
  });

  it('enforces declared response bounds and UTF-8', async () => {
    await expect(
      requestBytes(
        resolveConfig({
          fetch: recorder(
            response('tiny', { headers: { 'content-length': String(9 * 1024 * 1024) } }),
          ).fetch,
        }),
        '/x',
      ),
    ).rejects.toBeInstanceOf(ProtocolError);
    await expect(
      request(resolveConfig({ fetch: recorder(response(new Uint8Array([0xff]))).fetch }), '/x'),
    ).rejects.toBeInstanceOf(ProtocolError);
  });

  it('converts transport failures without hiding SDK failures', async () => {
    const failedFetch = vi.fn(() => Promise.reject(new Error('offline')));
    await expect(
      request(
        resolveConfig({ fetch: failedFetch as unknown as typeof globalThis.fetch }),
        '/ready',
      ),
    ).rejects.toBeInstanceOf(TransportError);
    await expect(
      withOperationDeadline(resolveConfig(), {}, () => Promise.reject(new ProtocolError('bad'))),
    ).rejects.toBeInstanceOf(ProtocolError);
  });
});

describe('problem documents', () => {
  it('returns a generic API error with correlation and retry metadata', async () => {
    const failure = await errorFromResponse(
      response('{"status":429,"detail":"slow down","code":"rate_limited"}', {
        status: 429,
        headers: { 'retry-after': '17', 'x-request-id': 'response-limit-801' },
      }),
    );
    expect(failure).toMatchObject({
      status: 429,
      code: 'rate_limited',
      message: 'slow down',
      requestId: 'response-limit-801',
      retryAfterMs: 17_000,
      isRetryable: true,
    });
    expect(failure).toBeInstanceOf(ApiError);
    expect((failure as ApiError).isUnauthenticated).toBe(false);
  });

  it('accepts a title fallback and a future HTTP date', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date('2026-09-05T00:00:00Z'));
    try {
      const failure = await errorFromResponse(
        response('{"title":"temporarily unavailable"}', {
          status: 503,
          headers: { 'retry-after': 'Sat, 05 Sep 2026 00:00:09 GMT' },
        }),
      );
      expect(failure).toMatchObject({ retryAfterMs: 9_000, isRetryable: true });
    } finally {
      vi.useRealTimers();
    }
  });

  it('classifies authentication and permanent API errors', () => {
    expect(new ApiError(401, 'sign in').isUnauthenticated).toBe(true);
    expect(new ApiError(403, 'forbidden').isUnauthenticated).toBe(false);
    expect(new ApiError(400, 'invalid').isRetryable).toBe(false);
  });

  it.each([
    ['not json', 500],
    ['[]', 500],
    ['{"status":400,"detail":"mismatch"}', 500],
    ['{"code":"missing_detail"}', 500],
  ])('rejects malformed problem document %s', async (body, status) => {
    await expect(errorFromResponse(response(body, { status }))).resolves.toBeInstanceOf(
      ProtocolError,
    );
  });

  it.each(['0', '-1', 'invalid', '999999999999999999'])(
    'ignores unusable Retry-After value %s',
    async (retryAfter) => {
      const failure = await errorFromResponse(
        response('{"detail":"wait"}', { status: 429, headers: { 'retry-after': retryAfter } }),
      );
      expect((failure as ApiError).retryAfterMs).toBeUndefined();
    },
  );
});

describe('deadlines and cancellation', () => {
  it('rejects invalid and expired deadlines', async () => {
    await expect(
      withOperationDeadline(resolveConfig(), { timeoutMs: 0 }, () => Promise.resolve('no')),
    ).rejects.toBeInstanceOf(ValidationError);
    await expect(
      withOperationDeadline(resolveConfig(), { timeoutMs: 5 }, () => new Promise(() => {})),
    ).rejects.toBeInstanceOf(TimeoutError);
  });

  it('honours caller cancellation before and during work', async () => {
    const before = new AbortController();
    before.abort('already cancelled');
    await expect(
      withOperationDeadline(resolveConfig(), { signal: before.signal }, () => Promise.resolve(1)),
    ).rejects.toBeInstanceOf(RequestAborted);

    const during = new AbortController();
    const pending = withOperationDeadline(
      resolveConfig(),
      { signal: during.signal },
      () => new Promise(() => {}),
    );
    during.abort(new Error('stop'));
    await expect(pending).rejects.toBeInstanceOf(RequestAborted);
  });

  it('exposes direct abort helpers', async () => {
    const controller = new AbortController();
    controller.abort();
    expect(() => throwIfSignalAborted(controller.signal)).toThrow();
    await expect(
      raceWithAbortSignal(controller.signal, () => Promise.resolve(1)),
    ).rejects.toThrow();
  });
});
