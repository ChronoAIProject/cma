import { describe, expect, it, vi } from 'vitest';
import {
  ApiError,
  ProtocolError,
  ValidationError,
  createClient,
  errorFromResponse,
} from '../src/index.js';
import { context, type ContractCase } from './public-contract/context.js';
import { resourceDispatch } from './public-contract/resources.js';
import {
  expectation,
  expectedProjection,
  expectedRequestUrl,
  problemBody,
  problemExtensions,
} from './public-contract/expectations.js';
import { retainedCorpora } from './retained-clients/fixtures.js';

const releases = retainedCorpora();

function exchange(fixture: ContractCase) {
  const fetch = vi.fn(async (url: string | URL | Request, init?: RequestInit) => {
    const expected = expectedRequestUrl(fixture);
    const actual = new URL(String(url));
    expect(actual.origin).toBe('https://contract.example');
    expect(actual.pathname).toBe(expected.pathname);
    expect([...actual.searchParams].sort()).toEqual([...expected.searchParams].sort());
    expect(init?.method).toBe(fixture.request.method);
    const headers = new Headers(init?.headers);
    for (const [key, value] of Object.entries(fixture.request.headers))
      expect(headers.get(key)).toBe(value);
    expect(init?.body === undefined ? undefined : JSON.parse(String(init.body))).toEqual(
      fixture.request.body,
    );
    return response(fixture);
  });
  const client = createClient({ baseUrl: 'https://contract.example', fetch });
  return { fetch, invoke: () => resourceDispatch[fixture.operation]!(context(client, fixture)) };
}
function response(fixture: ContractCase) {
  return new Response(
    fixture.response.bodyKind === 'empty' ? null : JSON.stringify(fixture.response.body),
    { status: fixture.response.status, headers: fixture.response.headers },
  );
}

it('pins two released wire baselines and all 112 cases without a network transport', () => {
  expect(releases.map(({ commit }) => commit)).toEqual([
    '93d4a199a3d36d47f5a89f9117d2bc735900bf4c',
    '4f03c35a80322673ec00df7a3b6b9399015f5a68',
  ]);
  expect(releases.flatMap(({ cases }) => cases)).toHaveLength(112);
});

for (const release of releases)
  describe(`retained ${release.name} API wire shapes`, () => {
    for (const fixture of release.cases)
      it(`${fixture.id}`, async () => {
        const mode = expectation(fixture);
        expect(['wire', 'preflight-validation', 'raw-wire-only']).toContain(mode);
        const { fetch, invoke } = exchange(fixture);
        const failure = problemBody(fixture);
        if (mode === 'raw-wire-only') {
          expect(await errorFromResponse(response(fixture))).toMatchObject({
            status: fixture.response.status,
            code: failure.code,
          });
          expect(fetch).not.toHaveBeenCalled();
        } else if (mode === 'preflight-validation') {
          await expect(Promise.resolve().then(invoke)).rejects.toBeInstanceOf(ValidationError);
          expect(fetch).not.toHaveBeenCalled();
          expect(await errorFromResponse(response(fixture))).toMatchObject({
            status: fixture.response.status,
            code: failure.code,
          });
        } else if (fixture.response.status >= 400) {
          const error = await Promise.resolve()
            .then(invoke)
            .catch((cause: unknown) => cause);
          expect(error).toBeInstanceOf(ApiError);
          expect(error).toMatchObject({
            status: fixture.response.status,
            code: failure.code,
            extensions: problemExtensions(fixture),
          });
          expect(fetch).toHaveBeenCalledOnce();
        } else {
          const result = await invoke();
          expect(result === undefined ? undefined : JSON.parse(JSON.stringify(result))).toEqual(
            expectedProjection(fixture),
          );
          expect(fetch).toHaveBeenCalledOnce();
        }
      });
  });

it.each(['missing-id', 'unknown-status', 'wrong-desktop-url'])(
  'rejects incompatible response mutation %s in the real current client decoder',
  async (mutation) => {
    const id = mutation === 'wrong-desktop-url' ? 'desktop-discovery' : 'agent-read';
    const fixture = structuredClone(releases[0]!.cases.find((item) => item.id === id)!);
    const body = fixture.response.body as Record<string, unknown>;
    if (mutation === 'missing-id') delete body.id;
    if (mutation === 'unknown-status') body.status = 'not_a_supported_status';
    if (mutation === 'wrong-desktop-url') body.websocket_url = 42;
    const { fetch, invoke } = exchange(fixture);
    await expect(Promise.resolve().then(invoke)).rejects.toBeInstanceOf(ProtocolError);
    expect(fetch).toHaveBeenCalledOnce();
  },
);

it('tolerates an additive ordinary response field without turning it into client authority', async () => {
  const fixture = structuredClone(releases[0]!.cases.find((item) => item.id === 'agent-read')!);
  const expected = structuredClone(expectedProjection(fixture));
  (fixture.response.body as Record<string, unknown>).future_unrecognized_field = 'ignored';
  const { invoke } = exchange(fixture);
  expect(await invoke()).toEqual(expected);
});
