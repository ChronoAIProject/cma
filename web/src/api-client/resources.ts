import type { ResolvedConfig } from './config.js';
import { ProtocolError, ValidationError } from './errors.js';
import { request, type OperationOptions, type RequestOptions } from './http.js';
import { array, object, optional, text, type Parser } from './schema.js';

export interface CommandOptions extends OperationOptions {
  readonly idempotencyKey: string;
}

export interface PageOptions extends OperationOptions {
  readonly cursor?: string;
  readonly limit?: number;
}

export interface EventOptions extends OperationOptions {
  readonly after?: number;
  readonly limit?: number;
}

export interface ResourcePage<Value, Cursor = string> {
  readonly items: readonly Value[];
  readonly next?: Cursor;
}

export function page<Value>(parse: Parser<Value>): Parser<ResourcePage<Value>> {
  return object({ items: array(parse), next: optional(text) });
}

export function segment(value: string): string {
  if (typeof value !== 'string' || value === '' || value === '.' || value === '..')
    throw new ValidationError('resource identifiers must be nonempty path segments');
  try {
    return encodeURIComponent(value);
  } catch (cause) {
    throw new ValidationError('resource identifier is not valid Unicode', { cause });
  }
}

export function query(
  path: string,
  values: Readonly<Record<string, string | number | undefined>>,
): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(values)) {
    if (value === undefined) continue;
    if (typeof value === 'number' && (!Number.isSafeInteger(value) || value < 0))
      throw new ValidationError(`${key} must be a nonnegative safe integer`);
    params.set(key, String(value));
  }
  const encoded = params.toString();
  return encoded === '' ? path : `${path}?${encoded}`;
}

export function command(options: CommandOptions): RequestOptions {
  const key = options.idempotencyKey;
  if (typeof key !== 'string' || !/^[\x21-\x7e]{1,255}$/.test(key))
    throw new ValidationError(
      'idempotencyKey must contain 1 to 255 visible ASCII characters without spaces',
    );
  return { ...options, headers: { 'Idempotency-Key': key } };
}

export function input<Value>(parse: Parser<Value>, value: Value): Value {
  try {
    parse(value);
  } catch (cause) {
    throw new ValidationError(cause instanceof Error ? cause.message : 'invalid request body', {
      cause,
    });
  }
  return value;
}

export async function read<Value>(
  config: ResolvedConfig,
  path: string,
  parse: Parser<Value>,
  options: RequestOptions = {},
): Promise<Value> {
  return parse(await request(config, path, { expectedStatus: 200, ...options }));
}

export function identity<Value extends { readonly id: string }>(
  parse: Parser<Value>,
  id: string,
): Parser<Value> {
  return (value) => {
    const result = parse(value);
    if (result.id !== id)
      throw new ProtocolError('response resource identity did not match the request');
    return result;
  };
}
