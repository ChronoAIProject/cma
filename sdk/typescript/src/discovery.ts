import type { ResolvedConfig } from './config.js';
import { ProtocolError } from './errors.js';
import { request, type OperationOptions } from './http.js';

/** Public state exposed while the replacement CMA interfaces are rebuilt. */
export interface Conformance {
  readonly service: string;
  readonly status: string;
  readonly interfaces: readonly string[];
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
