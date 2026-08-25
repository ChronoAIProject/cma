import type { ResolvedConfig } from './config.js';
import { ProtocolError } from './errors.js';
import { request, type OperationOptions } from './http.js';

/** Protocol classes and exact build served by one CMA deployment. */
export interface Conformance {
  readonly conformsTo: readonly string[];
  readonly version: string;
  readonly buildSha: string;
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
  return toConformance(await request(anonymous, '/api/v1/conformance', options));
}

/** Decode the public discovery document without accepting partial build identity. */
export function toConformance(raw: unknown): Conformance {
  const body = objectOf(raw, 'conformance document');
  const allowed = new Set(['conformsTo', 'version', 'build_sha']);
  const unexpected = Object.keys(body).find((key) => !allowed.has(key));
  if (unexpected !== undefined) {
    throw new ProtocolError(`conformance document carried unexpected field '${unexpected}'`);
  }
  if (!Array.isArray(body.conformsTo)) {
    throw new ProtocolError('conformance document conformsTo was not an array');
  }
  if (body.conformsTo.length === 0) {
    throw new ProtocolError('conformance document conformsTo was empty');
  }
  const conformsTo = body.conformsTo.map((entry, index) =>
    nonEmptyText(entry, `conformance document conformsTo[${String(index)}]`),
  );
  return {
    conformsTo,
    version: nonEmptyText(body.version, 'conformance document version'),
    buildSha: nonEmptyText(body.build_sha, 'conformance document build_sha'),
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
