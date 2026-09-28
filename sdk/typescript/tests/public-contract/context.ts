import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { readdirSync, readFileSync } from 'node:fs';
import type { Cma, CommandOptions } from '../../src/index.js';

export interface ContractCase {
  id: string;
  operation: string;
  scenario: string;
  request: {
    method: string;
    path: string;
    headers: Record<string, string>;
    body?: unknown;
    rawBody?: string;
    rawBodyRepeat?: number;
  };
  response: {
    status: number;
    headers: Record<string, string>;
    bodyKind: 'json' | 'empty' | 'text' | 'sse';
    streamPrefix?: boolean;
    body: unknown;
  };
  captures: Record<string, string>;
  headerCaptures?: Record<string, string>;
}

export interface DispatchContext {
  client: Cma;
  fixture: ContractCase;
  param(name: string): string;
  // Fixture JSON crosses the same runtime input validators as ordinary typed callers.
  body: never;
  options: never;
  command: CommandOptions;
  query: URLSearchParams;
}
export type Dispatch = (context: DispatchContext) => unknown;

export function expandRawRequest(request: ContractCase['request']): ContractCase['request'] {
  assert(
    request.body === undefined || request.rawBody === undefined,
    'body and rawBody are exclusive',
  );
  assert(
    request.rawBodyRepeat === undefined || request.rawBody !== undefined,
    'rawBodyRepeat requires rawBody',
  );
  if (request.rawBody === undefined) return request;
  assert.equal(typeof request.rawBody, 'string');
  const count = request.rawBodyRepeat ?? 1;
  assert(Number.isSafeInteger(count) && count >= 1 && count <= 2_097_153, 'Invalid rawBodyRepeat');
  assert(
    Buffer.byteLength(request.rawBody, 'utf8') * count <= 8_388_608,
    'Expanded rawBody exceeds 8 MiB',
  );
  return { ...request, rawBody: request.rawBody.repeat(count) };
}

function pointer(value: unknown, path: string): unknown {
  assert(path === '' || path.startsWith('/'), `Invalid JSON pointer ${path}`);
  for (const part of path === '' ? [] : path.slice(1).split('/')) {
    const key = part.replace(/~1/g, '/').replace(/~0/g, '~');
    assert(
      value !== null && typeof value === 'object' && Object.hasOwn(value, key),
      `Missing capture ${path}`,
    );
    value = (value as Record<string, unknown>)[key];
  }
  assert(
    value !== undefined && value !== null && typeof value !== 'object',
    `Capture must be a scalar: ${path}`,
  );
  return value;
}

export function resolveSymbols(value: unknown, symbols: Map<string, unknown>): unknown {
  if (typeof value === 'string') {
    const whole = /^\$([A-Za-z_][A-Za-z0-9_]*)$/.exec(value);
    if (whole) {
      assert(symbols.has(whole[1]!), `Unknown symbol ${whole[1]}`);
      return symbols.get(whole[1]!);
    }
    return value.replace(/\$([A-Za-z_][A-Za-z0-9_]*)/g, (_, key: string) => {
      assert(symbols.has(key), `Unknown symbol ${key}`);
      const found = symbols.get(key);
      assert(typeof found === 'string' || typeof found === 'number', `Non-text symbol ${key}`);
      return String(found);
    });
  }
  if (Array.isArray(value)) return value.map((item) => resolveSymbols(item, symbols));
  if (value !== null && typeof value === 'object')
    return Object.fromEntries(
      Object.entries(value).map(([key, item]) => [key, resolveSymbols(item, symbols)]),
    );
  return value;
}

export function loadCorpus(): ContractCase[] {
  const directory = new URL(
    '../../../../api/crates/layer1-public/tests/fixtures/public-contract/',
    import.meta.url,
  );
  const files = readdirSync(directory)
    .filter((name) => name.endsWith('.json'))
    .sort();
  for (const required of [
    'profiles.json',
    'instances.json',
    'agents.json',
    'admin.json',
    'defaults.json',
    'triggers.json',
  ])
    assert(files.includes(required), `Missing shared corpus ${required}`);
  const ids = new Set<string>();
  return files.flatMap((file) => {
    const raw = JSON.parse(readFileSync(new URL(file, directory), 'utf8')) as {
      version: number;
      cases: ContractCase[];
    };
    assert.equal(raw.version, 1, file);
    assert(Array.isArray(raw.cases) && raw.cases.length > 0, `Empty corpus ${file}`);
    const cases = raw.cases.map((fixture) => ({
      ...fixture,
      captures: fixture.captures ?? {},
      request: { ...fixture.request, headers: fixture.request.headers ?? {} },
    }));
    const symbols = new Map<string, Map<string, unknown>>();
    for (const fixture of cases) {
      assert(
        typeof fixture.id === 'string' && !ids.has(fixture.id),
        `Duplicate/missing case ${fixture.id}`,
      );
      ids.add(fixture.id);
      let scope = symbols.get(fixture.scenario);
      if (!scope)
        symbols.set(
          fixture.scenario,
          (scope = new Map<string, unknown>([
            ['ephemeral_credential', randomUUID()],
            ['interaction_expiry', Math.floor(Date.now() / 1000) + 3600],
          ])),
        );
      for (const [name, header] of Object.entries(fixture.headerCaptures ?? {})) {
        const value = fixture.response.headers[header];
        assert(typeof value === 'string', `Missing receipt header ${header}`);
        if (scope.has(name))
          assert.deepEqual(scope.get(name), value, `Conflicting capture ${name}`);
        scope.set(name, value);
      }
      for (const [name, path] of Object.entries(fixture.captures)) {
        const value = pointer(fixture.response.body, path);
        if (scope.has(name))
          assert.deepEqual(scope.get(name), value, `Conflicting capture ${name}`);
        scope.set(name, value);
      }
    }
    return cases.map((fixture) => {
      const scope = symbols.get(fixture.scenario)!;
      return {
        ...fixture,
        request: expandRawRequest(
          resolveSymbols(fixture.request, scope) as ContractCase['request'],
        ),
        response: resolveSymbols(fixture.response, scope) as ContractCase['response'],
      };
    });
  });
}

export function context(client: Cma, fixture: ContractCase): DispatchContext {
  const [method, template] = fixture.operation.split(' ');
  assert.equal(method, fixture.request.method);
  const url = new URL(fixture.request.path, 'https://contract.example');
  const parts = url.pathname.split('/');
  const route = template!.split('/');
  assert.equal(parts.length, route.length, fixture.id);
  const params = new Map<string, string>();
  route.forEach((part, index) => {
    const slot = /^\{([^}]+)\}$/.exec(part);
    if (slot) params.set(slot[1]!, decodeURIComponent(parts[index]!));
    else assert.equal(parts[index], part, fixture.id);
  });
  const options = Object.fromEntries(
    [...url.searchParams].map(([key, value]) => [
      key,
      ['limit', 'starting_after'].includes(key) ||
      (key === 'after' && !fixture.operation.endsWith('/items'))
        ? Number(value)
        : value,
    ]),
  );
  return {
    client,
    fixture,
    body: (fixture.request.rawBody ?? fixture.request.body) as never,
    options: options as never,
    command: { idempotencyKey: fixture.request.headers['idempotency-key']! },
    query: url.searchParams,
    param(name) {
      assert(params.has(name), `Missing parameter ${name} for ${fixture.operation}`);
      return params.get(name)!;
    },
  };
}
