import { documentDispatch } from './public-contract/documents.js';
import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import {
  createClient,
  ApiError,
  ProtocolError,
  ValidationError,
  errorFromResponse,
} from '../src/index.js';
import {
  context,
  expandRawRequest,
  loadCorpus,
  resolveSymbols,
  type ContractCase,
} from './public-contract/context.js';
import { resourceDispatch } from './public-contract/resources.js';
import { deliverableDispatch, deliveryExchange } from './public-contract/deliverables.js';
import { eventWorkerDispatch } from './public-contract/event-workers.js';
import { botDispatch } from './public-contract/bots.js';
import { operatorDispatch } from './public-contract/operator.js';
import { triggerDispatch } from './public-contract/triggers.js';
import { experienceDispatch } from './public-contract/experience.js';
import { agentEventsV2Dispatch } from './public-contract/agent-events-v2.js';
import { agentBrowserDispatch } from './public-contract/agent-browser.js';
import {
  expectation,
  expectedProjection,
  expectedRequestUrl,
  localCodes,
  problemBody,
  problemExtensions,
} from './public-contract/expectations.js';
import { browserOperations, verifyNavigation } from './public-contract/navigation.js';

const corpus = loadCorpus();
const dispatch = {
  ...deliverableDispatch(corpus),
  ...documentDispatch,
  ...botDispatch,
  ...eventWorkerDispatch,
  ...resourceDispatch,
  ...operatorDispatch,
  ...triggerDispatch,
  ...experienceDispatch,
  ...agentEventsV2Dispatch,
  ...agentBrowserDispatch,
};

function response(fixture: ContractCase): Response {
  const { bodyKind, body, status, headers } = fixture.response;
  if (bodyKind === 'sse') {
    const wire = (body as Record<string, unknown>[])
      .map(
        (frame) =>
          Object.entries(frame)
            .map(
              ([field, value]) =>
                `${field}: ${field === 'data' ? JSON.stringify(value) : String(value)}\n`,
            )
            .join('') + '\n',
      )
      .join('');
    return new Response(wire, { status, headers });
  }
  return new Response(
    bodyKind === 'empty' ? null : bodyKind === 'json' ? JSON.stringify(body) : String(body),
    { status, headers },
  );
}

function protocolFailureCodes(fixture: ContractCase): string[] {
  if (fixture.response.status !== 200) return [];
  if (fixture.response.bodyKind === 'json') {
    const body = fixture.response.body as {
      object?: string;
      status?: string;
      error?: { code: string };
    };
    return body.object === 'response' && body.status === 'failed' && body.error
      ? [body.error.code]
      : [];
  }
  if (fixture.response.bodyKind !== 'sse') return [];
  const frames = fixture.response.body as {
    event?: string;
    data: {
      type?: string;
      code?: string;
      response?: { status?: string; error?: { code: string } };
    };
  }[];
  return frames.flatMap(({ event, data }) => {
    if (data.type === 'RUN_ERROR' && data.code) return [data.code];
    if (
      event === 'response.failed' &&
      data.type === 'response.failed' &&
      data.response?.status === 'failed' &&
      data.response.error
    )
      return [data.response.error.code];
    return [];
  });
}

describe('shared public contract', () => {
  it('requires every loaded fixture to have a typed operation and every local refusal to exist', () => {
    for (const fixture of corpus)
      if (!browserOperations.has(fixture.operation))
        expect(dispatch[fixture.operation], fixture.operation).toBeTypeOf('function');
    for (const [id, code] of localCodes) {
      const fixture = corpus.find((entry) => entry.id === id);
      expect(fixture, id).toBeDefined();
      expect(problemBody(fixture!).code).toBe(code);
    }
  });

  it('replays every required protocol failure pair through a typed wire operation', () => {
    const inventory = JSON.parse(
      readFileSync(
        new URL(
          '../../../api/crates/layer1-public/tests/fixtures/public-contract-inventory.json',
          import.meta.url,
        ),
        'utf8',
      ),
    ) as { operations: Record<string, { required_protocol_failures?: string[] }> };
    const required = Object.entries(inventory.operations).flatMap(([operation, contract]) =>
      (contract.required_protocol_failures ?? []).map((code) => ({ operation, code })),
    );
    expect(required).toHaveLength(23);
    for (const { operation, code } of required) {
      const fixtures = corpus.filter(
        (fixture) =>
          fixture.operation === operation && protocolFailureCodes(fixture).includes(code),
      );
      expect(fixtures.length, `${operation}: ${code}`).toBeGreaterThan(0);
      for (const fixture of fixtures) {
        expect(dispatch[operation], operation).toBeTypeOf('function');
        expect(expectation(fixture), fixture.id).toBe('wire');
      }
    }
  });

  for (const fixture of corpus)
    it(`${expectation(fixture)}: ${fixture.id}: ${fixture.operation}`, async () => {
      const mode = expectation(fixture);
      const exchange = deliveryExchange(fixture, corpus);
      const fetch = vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
        const currentFixture = exchange[fetch.mock.calls.length - 1]!;
        expect(currentFixture, 'unexpected typed HTTP request').toBeDefined();
        const actual = new URL(String(url));
        const expected = expectedRequestUrl(currentFixture);
        expect(actual.pathname).toBe(expected.pathname);
        expect([...actual.searchParams].sort()).toEqual([...expected.searchParams].sort());
        expect(init?.method).toBe(currentFixture.request.method);
        const headers = new Headers(init?.headers);
        if (mode === 'serializer-protection' && currentFixture.id.endsWith('-missing-media-type'))
          expect(headers.get('content-type')).toBe('application/json');
        for (const [name, value] of Object.entries(currentFixture.request.headers)) {
          if (
            currentFixture.operation.endsWith('/entry-intent') &&
            ['origin', 'sec-fetch-site'].includes(name)
          ) {
            // Fetch metadata and Origin are supplied by the browser, not forged by the SDK.
            expect(headers.has(name)).toBe(false);
            expect(init?.credentials).toBe('include');
            continue;
          }
          if (name === 'cookie') {
            // Cookies belong to the browser/host jar, never to SDK-managed header values.
            expect(headers.has('cookie')).toBe(false);
            expect(init?.credentials).toBe(
              currentFixture.operation === 'GET /auth/signup-options' ? 'omit' : 'include',
            );
            continue;
          }
          expect(headers.get(name), name).toBe(value);
        }
        if (currentFixture.operation === 'POST /api/v1/agents/{id}/attachments') {
          expect(init?.body).toBeInstanceOf(Blob);
          expect(await (init?.body as Blob).text()).toBe(currentFixture.request.rawBody);
        } else {
          expect(init?.body === undefined ? undefined : JSON.parse(String(init.body))).toEqual(
            currentFixture.id === 'responses-conversation-content-type'
              ? {}
              : currentFixture.request.body,
          );
        }
        return response(currentFixture);
      });
      const client = createClient({
        baseUrl: 'https://contract.example',
        cookies:
          fixture.operation === 'GET /auth/session' || fixture.operation.endsWith('/entry-intent'),
        fetch,
      });
      if (mode === 'browser-only') {
        await verifyNavigation(client, fixture, response(fixture));
        expect(fetch).not.toHaveBeenCalled();
        return;
      }
      const invoke = async () => dispatch[fixture.operation]!(context(client, fixture));
      if (fixture.id === 'triggers-entry-intent-cross-origin') {
        // Browser-origin rejection is an empty HTTP refusal, not a domain problem document.
        expect(fixture.response.status).toBe(403);
        expect(fixture.response.bodyKind).toBe('empty');
        await expect(invoke()).rejects.toBeInstanceOf(ProtocolError);
        expect(fetch).toHaveBeenCalledTimes(exchange.length);
        return;
      }
      if (mode === 'raw-wire-only') {
        // Closed query/header shapes and raw padding cannot be emitted by these typed methods.
        const body = problemBody(fixture);
        const error = await errorFromResponse(response(fixture));
        expect(error).toBeInstanceOf(ApiError);
        expect(error).toMatchObject({
          status: fixture.response.status,
          code: body.code,
        });
        expect((error as ApiError).extensions).toEqual(problemExtensions(fixture));
        expect(fetch).not.toHaveBeenCalled();
        return;
      }
      if (mode === 'preflight-validation') {
        await expect(invoke()).rejects.toBeInstanceOf(ValidationError);
        expect(fetch).not.toHaveBeenCalled();
        const problem = await errorFromResponse(response(fixture));
        expect(problem).toBeInstanceOf(ApiError);
        expect(problem).toMatchObject({
          status: fixture.response.status,
          code: problemBody(fixture).code,
        });
      } else if (fixture.response.status >= 400 && fixture.response.bodyKind === 'empty') {
        await expect(invoke()).rejects.toBeInstanceOf(ProtocolError);
        expect(fetch).toHaveBeenCalledTimes(exchange.length);
      } else if (
        fixture.response.status >= 400 &&
        !(fixture.operation === 'GET /auth/session' && fixture.response.status === 401)
      ) {
        const body = problemBody(fixture);
        const result = await invoke().then(
          () => {
            throw new Error('Expected named refusal');
          },
          (error: unknown) => error,
        );
        expect(result).toBeInstanceOf(ApiError);
        expect(result).toMatchObject({ status: fixture.response.status, code: body.code });
        expect((result as ApiError).extensions).toEqual(problemExtensions(fixture));
        expect(fetch).toHaveBeenCalledTimes(exchange.length);
      } else {
        const result = await invoke();
        expect(result === undefined ? undefined : JSON.parse(JSON.stringify(result))).toEqual(
          expectedProjection(fixture),
        );
        expect(fetch).toHaveBeenCalledTimes(exchange.length);
      }
    });

  it('rejects unknown symbols and preserves captured scalar types', () => {
    const symbols = new Map<string, unknown>([
      ['id', 'agt_fixture'],
      ['seq', 4],
    ]);
    expect(resolveSymbols({ url: '/agents/$id', seq: '$seq' }, symbols)).toEqual({
      url: '/agents/agt_fixture',
      seq: 4,
    });
    expect(() => resolveSymbols('$missing', symbols)).toThrow('Unknown symbol');
    expect(() => resolveSymbols('/agents/$missing', symbols)).toThrow('Unknown symbol');
  });

  it('classifies the unsafe Trigger version as local validation, never an exact wire replay', () => {
    const fixture = corpus.find((entry) => entry.id === 'trigger-state-version-overflow')!;
    expect(expectation(fixture)).toBe('preflight-validation');
    const body = fixture.request.body as Record<string, unknown>;
    expect(Number.isSafeInteger(body.draft_version)).toBe(false);
    expect(() =>
      expectation({
        ...fixture,
        request: { ...fixture.request, body: { ...body, draft_version: Number.MAX_SAFE_INTEGER } },
      }),
    ).toThrow('Expected an unsafe integer fixture');
  });

  it('bounds compact raw-wire requests before expansion and rejects ambiguous body definitions', () => {
    const request = { method: 'POST', path: '/api/v1/admin/settings', headers: {} };
    expect(expandRawRequest({ ...request, rawBody: ' ', rawBodyRepeat: 3 }).rawBody).toBe('   ');
    for (const rawBodyRepeat of [0, -1, 1.5, 2_097_154])
      expect(() => expandRawRequest({ ...request, rawBody: ' ', rawBodyRepeat })).toThrow(
        'Invalid rawBodyRepeat',
      );
    expect(() => expandRawRequest({ ...request, body: {}, rawBody: '{}' })).toThrow('exclusive');
    expect(() => expandRawRequest({ ...request, rawBodyRepeat: 2 })).toThrow('requires rawBody');
    expect(() =>
      expandRawRequest({ ...request, rawBody: '\u00e9\u00e9', rawBodyRepeat: 2_097_153 }),
    ).toThrow('exceeds 8 MiB');
  });
});
