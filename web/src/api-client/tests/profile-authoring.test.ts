// @vitest-environment node
import { expect, it } from 'vitest';
import {
  ApiError,
  createClient,
  ProtocolError,
  RequestAborted,
  TransportError,
  ValidationError,
  type Cma,
} from '../index.js';
import { transport } from './resource-fixtures.js';

const bounds = {
  min_cpu_millis: 100,
  max_cpu_millis: 8000,
  min_memory_mib: 128,
  max_memory_mib: 16384,
};
const skill = {
  id: 'review',
  name: 'Review',
  versions: ['999999999999999999.1', '2.10', '2.9', '0.0'],
};
const request = { url: 'https://github.com/owner/project.git/' };
const repository = {
  url: 'https://github.com/Owner/Project',
  default_branch: 'main',
  commit_sha: 'a'.repeat(40),
};

it('uses authenticated exact paths and retains literal versions, query encoding, and opaque pages', async () => {
  const t = transport();
  const client = createClient({
    baseUrl: 'https://cma.example.test/gateway',
    credential: () => 'sdk-fixture',
    fetch: t.fetch,
  });
  t.answer(bounds);
  await expect(client.profileAuthoring.bounds()).resolves.toEqual(bounds);
  t.answer([
    {
      id: 'github',
      catalog_id: 'canonical-github',
      detail_url: 'https://catalog.example.test/services/canonical-github',
      name: 'GitHub',
      description: 'Repository access',
      private_data: 'not projected',
    },
  ]);
  await expect(client.profileAuthoring.services()).resolves.toEqual([
    {
      id: 'github',
      catalog_id: 'canonical-github',
      detail_url: 'https://catalog.example.test/services/canonical-github',
      name: 'GitHub',
      description: 'Repository access',
    },
  ]);
  t.answer({ items: [skill], next: 'cursor/two+' });
  const page = await client.profileAuthoring.searchSkills({
    q: 'build & test/+',
    limit: 2,
    cursor: 'cursor/one+',
  });
  expect(page.items[0]?.versions).toEqual(skill.versions);
  expect(t.calls.at(-1)?.url).toBe(
    'https://cma.example.test/gateway/api/v1/profile-authoring/skills?q=build+%26+test%2F%2B&limit=2&cursor=cursor%2Fone%2B',
  );
  t.answer({ items: [] });
  await expect(client.profileAuthoring.searchSkills({ cursor: page.next! })).resolves.toEqual({
    items: [],
  });
  expect(t.calls.map((call) => new URL(call.url).pathname)).toEqual(
    ['bounds', 'services', 'skills', 'skills'].map(
      (path) => `/gateway/api/v1/profile-authoring/${path}`,
    ),
  );
  for (const { init } of t.calls) {
    const headers = new Headers(init.headers);
    expect(init.method).toBe('GET');
    expect(init.body).toBeUndefined();
    expect(headers.get('Authorization')).toBe('Bearer sdk-fixture');
    expect(headers.get('Idempotency-Key')).toBeNull();
  }
});

it('resolves repositories using a read-only POST and preserves omitted reference semantics', async () => {
  const t = transport();
  for (const reference of [undefined, 'release/v1']) {
    const body = { ...request, ...(reference === undefined ? {} : { reference }) };
    const resolved = { ...repository, ...(reference === undefined ? {} : { reference }) };
    t.answer(resolved);
    await expect(t.client.profileAuthoring.resolveRepository(body)).resolves.toEqual(resolved);
    const call = t.calls.at(-1)!;
    expect(call.url).toBe('https://cma.example.test/api/v1/profile-authoring/repositories/resolve');
    expect(call.init.method).toBe('POST');
    expect(JSON.parse(String(call.init.body))).toEqual(body);
    expect(new Headers(call.init.headers).get('Idempotency-Key')).toBeNull();
    expect(new Headers(call.init.headers).get('Content-Type')).toBe('application/json');
  }
  expect(t.calls).toHaveLength(2);
});

it('validates raw repository response grammar while leaving request admission to CMA', async () => {
  const t = transport();
  const urls = [
    'http://github.com/Owner/Project',
    'https://github.com//Owner/Project',
    'https://github.com/Owner//Project',
    'https://github.com/Owner/Project//',
    'https://github.com/Owner/./Project',
    'https://github.com/Owner/Other/../Project',
    'https://github.com/Owner/%2e/Project',
    'https://github.com/Owner/Project?query',
    'https://github.com/Owner/Project#fragment',
    'https://github.com/./Project',
    'https://github.com/Owner/..git',
    `https://github.com/${'o'.repeat(101)}/Project`,
    `https://github.com/Owner/${'r'.repeat(101)}.git/`,
  ];
  for (const url of urls) {
    for (const requestedUrl of [request.url, url]) {
      t.answer({ ...repository, url });
      await expect(
        t.client.profileAuthoring.resolveRepository({ url: requestedUrl }),
      ).rejects.toBeInstanceOf(ProtocolError);
    }
    t.answer({ status: 422, code: 'invalid_repository', detail: 'Repository refused' }, 422);
    await expect(t.client.profileAuthoring.resolveRepository({ url })).rejects.toMatchObject({
      status: 422,
      code: 'invalid_repository',
    });
  }
  expect(t.calls).toHaveLength(urls.length * 3);
});

it('normalizes one trailing slash and .git before checking the 100-byte repository bound', async () => {
  const t = transport();
  const identity = `${'o'.repeat(100)}/${'r'.repeat(100)}`;
  const resolved = { ...repository, url: `https://github.com/${identity}` };
  t.answer(resolved);
  await expect(
    t.client.profileAuthoring.resolveRepository({
      url: `https://github.com/${identity.toUpperCase()}.git/`,
    }),
  ).resolves.toEqual(resolved);
  expect(t.calls).toHaveLength(1);
});

it.each([
  { limit: 0 },
  { limit: 51 },
  { limit: 1.5 },
  { q: '\u00e9'.repeat(101) },
  { q: 'search\nline' },
  { cursor: '' },
  { cursor: 'x'.repeat(2049) },
  { cursor: 'cursor\u0085' },
])('rejects invalid skill search before transport: %j', async (query) => {
  const t = transport();
  await expect(t.client.profileAuthoring.searchSkills(query)).rejects.toBeInstanceOf(
    ValidationError,
  );
  expect(t.fetch).not.toHaveBeenCalled();
});

it('accepts exact UTF-8 input bounds and validates repository request shapes', async () => {
  const t = transport();
  t.answer({ items: [] });
  await expect(
    t.client.profileAuthoring.searchSkills({
      q: '\u00e9'.repeat(100),
      limit: 50,
      cursor: 'x'.repeat(2048),
    }),
  ).resolves.toEqual({ items: [] });
  for (const body of [null, {}, { url: 1 }, { url: request.url, reference: null }])
    await expect(t.client.profileAuthoring.resolveRepository(body as never)).rejects.toBeInstanceOf(
      ValidationError,
    );
  expect(t.calls).toHaveLength(1);
});

const invoke: Record<string, (client: Cma) => Promise<unknown>> = {
  bounds: (client) => client.profileAuthoring.bounds(),
  services: (client) => client.profileAuthoring.services(),
  skills: (client) => client.profileAuthoring.searchSkills(),
  'repositories/resolve': (client) => client.profileAuthoring.resolveRepository(request),
};

it.each(['services', 'skills'])(
  'applies the authoring-specific UTF-8 description bound to %s',
  async (path) => {
    const t = transport();
    const description = '\u754c'.repeat(2048);
    expect(new TextEncoder().encode(description)).toHaveLength(6144);
    for (const suffix of ['', 'a']) {
      const entry = { ...skill, description: description + suffix };
      const response = path === 'services' ? [entry] : { items: [entry] };
      t.answer(response);
      const result = invoke[path]!(t.client);
      if (suffix === '') await expect(result).resolves.toBeDefined();
      else await expect(result).rejects.toBeInstanceOf(ProtocolError);
    }
    expect(t.calls).toHaveLength(2);
  },
);

it.each([
  ['bounds', { ...bounds, min_cpu_millis: 9000 }],
  ['bounds', { ...bounds, min_cpu_millis: 0 }],
  ['bounds', { ...bounds, max_cpu_millis: 0x1_0000_0000 }],
  ['bounds', { min_cpu_millis: '100' }],
  [
    'services',
    [
      { id: 'same', name: 'One' },
      { id: 'same', name: 'Two' },
    ],
  ],
  ['services', [{ id: 'service', name: '' }]],
  ['services', [{ id: 'service', name: 'Name', description: null }]],
  ['services', { items: [] }],
  ['skills', { items: [{ ...skill, versions: ['01.2'] }] }],
  ['skills', { items: [{ ...skill, versions: [1.2] }] }],
  ['skills', { items: [{ ...skill, versions: ['2.9', '2.10'] }] }],
  ['skills', { items: [skill, skill] }],
  ['skills', { items: [skill], next: '' }],
  ['skills', { items: [skill], next: null }],
  ['repositories/resolve', { ...repository, url: 'https://github.com/Other/Project' }],
  ['repositories/resolve', { ...repository, commit_sha: 'A'.repeat(40) }],
  ['repositories/resolve', { ...repository, default_branch: '' }],
  ['repositories/resolve', { ...repository, reference: 'main' }],
  ['repositories/resolve', { ...repository, reference: null }],
] as const)('rejects malformed or inconsistent %s responses: %j', async (path, body) => {
  const t = transport();
  t.answer(body);
  await expect(invoke[path]!(t.client)).rejects.toBeInstanceOf(ProtocolError);
  expect(t.calls).toHaveLength(1);
});

it.each([
  ['bounds', 401, 'not_authenticated'],
  ['bounds', 403, 'forbidden'],
  ['services', 403, 'forbidden'],
  ['services', 503, 'profile_services_unavailable'],
  ['skills', 403, 'forbidden'],
  ['skills', 401, 'not_authenticated'],
  ['skills', 429, 'rate_limited'],
  ['skills', 400, 'invalid_profile_authoring_query'],
  ['skills', 503, 'profile_skills_unavailable'],
  ['repositories/resolve', 403, 'forbidden'],
  ['repositories/resolve', 422, 'invalid_repository'],
  ['repositories/resolve', 422, 'invalid_repository_reference'],
  ['repositories/resolve', 404, 'repository_not_found'],
  ['repositories/resolve', 404, 'repository_reference_not_found'],
  ['repositories/resolve', 503, 'repository_unavailable'],
] as const)('preserves the %s %i %s refusal without retry', async (path, status, code) => {
  const t = transport();
  t.fetch.mockResolvedValueOnce(
    new Response(
      JSON.stringify({ status, code, detail: 'Authoring refused', field: 'authoring' }),
      {
        status,
        headers: {
          'content-type': 'application/problem+json',
          'x-request-id': 'request-authoring',
          'retry-after': '3',
        },
      },
    ),
  );
  const error = await invoke[path]!(t.client).catch((error) => error as unknown);
  expect(error).toBeInstanceOf(ApiError);
  expect(error).toMatchObject({
    status,
    code,
    message: 'Authoring refused',
    requestId: 'request-authoring',
    retryAfterMs: 3000,
    extensions: { field: 'authoring' },
  });
  expect(t.calls).toHaveLength(0);
  expect(t.fetch).toHaveBeenCalledTimes(1);
});

it('requires HTTP 200 for all authoring operations', async () => {
  const t = transport();
  t.answer({}, 201);
  for (const call of Object.values(invoke))
    await expect(call(t.client)).rejects.toBeInstanceOf(ProtocolError);
  expect(t.calls).toHaveLength(4);
});

it('retains an advancing continuation across empty filtered skill pages', async () => {
  const t = transport();
  t.answer({ items: [], next: 'next-filtered' });
  await expect(t.client.profileAuthoring.searchSkills({ cursor: 'previous' })).resolves.toEqual({
    items: [],
    next: 'next-filtered',
  });
  await expect(
    t.client.profileAuthoring.searchSkills({ cursor: 'next-filtered' }),
  ).rejects.toBeInstanceOf(ProtocolError);
  expect(t.calls).toHaveLength(2);
});

it('accepts AbortSignal for the read-only POST and cancels without admission or retries', async () => {
  const t = transport();
  const controller = new AbortController();
  controller.abort();
  await expect(
    t.client.profileAuthoring.resolveRepository(request, { signal: controller.signal }),
  ).rejects.toBeInstanceOf(RequestAborted);
  expect(t.fetch).not.toHaveBeenCalled();
});

it('preserves skill catalog transport and protocol failures without replacing them with empty pages', async () => {
  const t = transport();
  t.fetch.mockRejectedValueOnce(new TypeError('Synthetic connection failure'));
  await expect(t.client.profileAuthoring.searchSkills()).rejects.toBeInstanceOf(TransportError);
  t.fetch.mockResolvedValueOnce(new Response('<html>synthetic upstream failure</html>'));
  await expect(t.client.profileAuthoring.searchSkills()).rejects.toBeInstanceOf(ProtocolError);
  expect(t.fetch).toHaveBeenCalledTimes(2);
});

it('aborts an in-flight skill page without retrying or accepting a late response', async () => {
  const t = transport();
  const controller = new AbortController();
  let resolve!: (response: Response) => void;
  let started!: () => void;
  const fetching = new Promise<void>((done) => {
    started = done;
  });
  t.fetch.mockImplementationOnce(
    () =>
      new Promise<Response>((done) => {
        resolve = done;
        started();
      }),
  );
  const result = t.client.profileAuthoring.searchSkills(
    { q: 'synthetic', cursor: 'original-page' },
    { signal: controller.signal },
  );
  const rejected = expect(result).rejects.toBeInstanceOf(RequestAborted);
  await fetching;
  controller.abort();
  await rejected;
  resolve(new Response(JSON.stringify({ items: [skill] })));
  expect(t.fetch).toHaveBeenCalledOnce();
});
