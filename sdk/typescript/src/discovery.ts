import type { ResolvedConfig } from './config.js';
import { ProtocolError } from './errors.js';
import { request, type OperationOptions } from './http.js';

/** Public state of the currently composed CMA interfaces. */
export interface Conformance {
  readonly service: string;
  readonly status: string;
  readonly interfaces: readonly string[];
}

/** The generated OpenAPI document, retaining its schema and operation extensions. */
export interface OpenApiDocument extends Readonly<Record<string, unknown>> {
  readonly openapi: string;
  readonly info: Readonly<Record<string, unknown>> & {
    readonly title: string;
    readonly version: string;
  };
  readonly paths: {
    readonly [path: `/${string}`]: Readonly<Record<string, unknown>>;
    readonly [extension: `x-${string}`]: unknown;
  };
}

export async function readOpenApi(
  config: ResolvedConfig,
  options: OperationOptions = {},
): Promise<OpenApiDocument> {
  const raw = objectOf(
    await request({ ...config, credential: undefined, cookies: false }, '/openapi.json', {
      ...options,
      expectedStatus: 200,
    }),
    'OpenAPI document',
  );
  const version = nonEmptyText(raw.openapi, 'OpenAPI version');
  if (!/^3\.\d+\.\d+$/.test(version)) throw new ProtocolError('expected OpenAPI version 3');
  const info = objectOf(raw.info, 'OpenAPI info');
  const paths = Object.fromEntries(
    Object.entries(objectOf(raw.paths, 'OpenAPI paths')).map(([path, value]) => {
      if (path.startsWith('x-')) return [path, value];
      if (!path.startsWith('/')) throw new ProtocolError('OpenAPI path must start with /');
      return [path, objectOf(value, 'OpenAPI path item')];
    }),
  ) as OpenApiDocument['paths'];
  return {
    ...raw,
    openapi: version,
    info: {
      ...info,
      title: nonEmptyText(info.title, 'OpenAPI title'),
      version: nonEmptyText(info.version, 'OpenAPI info version'),
    },
    paths,
  };
}

/**
 * Discover a deployment without presenting the configured bearer credential.
 *
 * Correlation and response observation remain enabled: public means anonymous,
 * not invisible to the caller's diagnostics.
 */
export async function readConformance(
  config: ResolvedConfig,
  options: OperationOptions = {},
): Promise<Conformance> {
  const anonymous = { ...config, credential: undefined, cookies: false };
  return toConformance(
    await request(anonymous, '/api/v1/conformance', { ...options, expectedStatus: 200 }),
  );
}

/** Decode the public discovery document without accepting ambiguous fields. */
export function toConformance(raw: unknown): Conformance {
  const body = objectOf(raw, 'conformance document');
  const allowed = new Set(['service', 'status', 'interfaces']);
  const unexpected = Object.keys(body).find((key) => !allowed.has(key));
  if (unexpected !== undefined) {
    throw new ProtocolError(`conformance document carried unexpected field '${unexpected}'`);
  }
  if (!Array.isArray(body.interfaces)) {
    throw new ProtocolError('conformance document interfaces was not an array');
  }
  const interfaces = body.interfaces.map((entry, index) =>
    nonEmptyText(entry, `conformance document interfaces[${String(index)}]`),
  );
  return {
    service: nonEmptyText(body.service, 'conformance document service'),
    status: nonEmptyText(body.status, 'conformance document status'),
    interfaces,
  };
}

function objectOf(raw: unknown, name: string): Record<string, unknown> {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    throw new ProtocolError(`${name} was not an object`);
  }
  return raw as Record<string, unknown>;
}

function nonEmptyText(raw: unknown, name: string): string {
  if (typeof raw !== 'string' || raw.trim() === '') {
    throw new ProtocolError(`${name} was not a non-empty string`);
  }
  return raw;
}
