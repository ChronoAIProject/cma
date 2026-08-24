import { headersFor, urlOf, type ResolvedConfig } from './config.js';
import {
  ApiError,
  CapabilityRefused,
  CmaError,
  DeploymentAtCapacity,
  ProtocolError,
  RequestAborted,
  SessionLimitReached,
  TimeoutError,
  TransportError,
  ValidationError,
} from './errors.js';

const MAX_RESPONSE_BYTES = 8 * 1024 * 1024;

/** Deadline and cancellation controls shared by every operation. */
export interface OperationOptions {
  readonly signal?: AbortSignal;
  readonly timeoutMs?: number;
}

export interface RequestOptions extends OperationOptions {
  readonly method?: string;
  readonly body?: unknown;
  readonly headers?: HeadersInit;
  readonly expectedStatus?: number | readonly number[];
}

/** Open and validate an HTTP response without consuming its successful body. */
export async function requestResponse(
  config: ResolvedConfig,
  path: string,
  options: RequestOptions = {},
): Promise<Response> {
  return withDeadline(config, options, async (signal) => {
    const response = await send(config, path, options, signal);
    await requireSuccess(response, options.expectedStatus);
    return response;
  });
}

/** Make one JSON request and validate that its body is JSON. */
export async function request(
  config: ResolvedConfig,
  path: string,
  options: RequestOptions = {},
): Promise<unknown> {
  return withDeadline(config, options, async (signal) => {
    const response = await send(config, path, options, signal);
    await requireSuccess(response, options.expectedStatus);
    if (response.status === 204) {
      return null;
    }
    const text = await readText(response);
    if (text.trim() === '') {
      return null;
    }
    try {
      return JSON.parse(text) as unknown;
    } catch (cause) {
      throw new ProtocolError('success response was not valid JSON', { cause });
    }
  });
}

/** Walk one array collection through the server's RFC 8288 next links. */
export async function requestPages<T>(
  config: ResolvedConfig,
  path: string,
  decodePage: (body: unknown) => readonly T[],
  options: OperationOptions = {},
): Promise<readonly T[]> {
  const collectionPath = path.split('?', 1)[0];
  if (collectionPath === undefined || !collectionPath.startsWith('/')) {
    throw new ValidationError('paginated request paths must begin with a slash');
  }
  const visited = new Set<string>();
  const items: T[] = [];
  let next: string | undefined = path;
  while (next !== undefined) {
    if (visited.has(next)) {
      throw new ProtocolError('pagination repeated a continuation link');
    }
    visited.add(next);
    const response = await requestResponse(config, next, { ...options, expectedStatus: 200 });
    items.push(...decodePage(await responseJson(response)));
    next = nextPagePath(response.headers.get('link'), collectionPath);
  }
  return items;
}

/** Make one request whose successful response must not carry a body. */
export async function requestVoid(
  config: ResolvedConfig,
  path: string,
  options: RequestOptions = {},
): Promise<void> {
  await withDeadline(config, options, async (signal) => {
    const response = await send(config, path, options, signal);
    await requireSuccess(response, options.expectedStatus);
    const text = await readText(response);
    if (text.trim() !== '') {
      throw new ProtocolError(`HTTP ${String(response.status)} unexpectedly carried a body`);
    }
  });
}

/** Make one request whose successful body is opaque bytes. */
export async function requestBytes(
  config: ResolvedConfig,
  path: string,
  options: RequestOptions = {},
): Promise<Uint8Array> {
  return withDeadline(config, options, async (signal) => {
    const response = await send(config, path, options, signal);
    await requireSuccess(response, options.expectedStatus);
    return readBytes(response);
  });
}

/** Execute a fetch-backed operation under one caller-visible deadline. */
export async function withDeadline<T>(
  config: ResolvedConfig,
  options: OperationOptions,
  operation: (signal: AbortSignal) => Promise<T>,
): Promise<T> {
  const timeoutMs = options.timeoutMs ?? config.requestTimeoutMs;
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0) {
    throw new ValidationError('timeoutMs must be a positive safe integer');
  }
  if (options.signal?.aborted === true) {
    throw new RequestAborted(undefined, { cause: options.signal.reason });
  }

  const controller = new AbortController();
  let timedOut = false;
  const onAbort = () => controller.abort(options.signal?.reason);
  options.signal?.addEventListener('abort', onAbort, { once: true });
  const timer = setTimeout(() => {
    timedOut = true;
    controller.abort();
  }, timeoutMs);

  try {
    return await operation(controller.signal);
  } catch (cause) {
    if (cause instanceof CmaError) {
      throw cause;
    }
    if (isAborted(options.signal)) {
      throw new RequestAborted(undefined, { cause });
    }
    if (timedOut) {
      throw new TimeoutError(timeoutMs, { cause });
    }
    throw new TransportError(`cannot reach the control plane: ${messageOf(cause)}`, { cause });
  } finally {
    clearTimeout(timer);
    options.signal?.removeEventListener('abort', onAbort);
  }
}

function isAborted(signal: AbortSignal | undefined): boolean {
  return signal?.aborted === true;
}

async function send(
  config: ResolvedConfig,
  path: string,
  options: RequestOptions,
  signal: AbortSignal,
): Promise<Response> {
  let body: string | undefined;
  if (options.body !== undefined) {
    try {
      body = JSON.stringify(options.body);
    } catch (cause) {
      throw new ValidationError('request body could not be encoded as JSON', { cause });
    }
    if (body === undefined) {
      throw new ValidationError('request body could not be encoded as JSON');
    }
  }
  return config.fetch(urlOf(config, path), {
    method: options.method ?? 'GET',
    headers: await headersFor(config, options.headers),
    credentials: config.cookies ? 'include' : 'omit',
    body,
    signal,
  });
}

async function responseJson(response: Response): Promise<unknown> {
  const text = await readText(response);
  if (text.trim() === '') {
    return null;
  }
  try {
    return JSON.parse(text) as unknown;
  } catch (cause) {
    throw new ProtocolError('success response was not valid JSON', { cause });
  }
}

function nextPagePath(link: string | null, collectionPath: string): string | undefined {
  if (link === null) {
    return undefined;
  }
  for (const entry of link.split(',')) {
    const match = /^\s*<([^>]*)>(.*)$/.exec(entry);
    if (match === null) {
      continue;
    }
    const target = match[1];
    const parameters = match[2] ?? '';
    const relation = /(?:^|;)\s*rel\s*=\s*(?:"([^"]*)"|([^;\s]+))/i.exec(parameters);
    const names = (relation?.[1] ?? relation?.[2] ?? '').split(/\s+/);
    if (!names.some((name) => name.toLowerCase() === 'next')) {
      continue;
    }
    if (target === undefined || !target.startsWith(`${collectionPath}?`) || target.includes('#')) {
      throw new ProtocolError('pagination next link left its collection');
    }
    return target;
  }
  throw new ProtocolError('pagination Link header carried no valid next relation');
}

async function requireSuccess(
  response: Response,
  expected: number | readonly number[] | undefined,
): Promise<void> {
  if (!response.ok) {
    throw await errorFromResponse(response);
  }
  if (expected === undefined) {
    return;
  }
  const statuses = typeof expected === 'number' ? [expected] : expected;
  if (!statuses.includes(response.status)) {
    throw new ProtocolError(
      `operation expected HTTP ${statuses.join(' or ')} but received ${String(response.status)}`,
    );
  }
}

/** Decode one RFC 9457-style failure without inferring types from prose. */
export async function errorFromResponse(response: Response): Promise<ApiError | ProtocolError> {
  const text = await readText(response);
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch (cause) {
    return new ProtocolError(
      `HTTP ${String(response.status)} did not carry a machine-readable problem document`,
      { cause },
    );
  }
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    return new ProtocolError(`HTTP ${String(response.status)} problem document was not an object`);
  }

  const body = raw as Record<string, unknown>;
  if (typeof body.status === 'number' && body.status !== response.status) {
    return new ProtocolError(
      `problem status ${String(body.status)} did not match HTTP ${String(response.status)}`,
    );
  }
  const code = nonEmptyText(body.code);
  const detail = nonEmptyText(body.detail) ?? nonEmptyText(body.title);
  if (detail === undefined) {
    return new ProtocolError(`HTTP ${String(response.status)} problem document carried no detail`);
  }

  if (code === 'capability_unsupported') {
    const capability = nonEmptyText(body.capability);
    return capability === undefined
      ? new ProtocolError('capability_unsupported problem omitted its typed capability field')
      : new CapabilityRefused(response.status, detail, capability);
  }
  if (code === 'session_limit_reached') {
    return new SessionLimitReached(
      response.status,
      detail,
      nonNegativeInteger(body.limit),
      nonNegativeInteger(body.live),
    );
  }
  if (code === 'deployment_at_capacity') {
    return new DeploymentAtCapacity(
      response.status,
      detail,
      nonNegativeInteger(body.requested_cpu_millis),
      nonNegativeInteger(body.requested_memory_mib),
      nonNegativeInteger(body.free_cpu_millis),
      nonNegativeInteger(body.free_memory_mib),
    );
  }
  return new ApiError(response.status, detail, code);
}

async function readText(response: Response): Promise<string> {
  const bytes = await readBytes(response);
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch (cause) {
    throw new ProtocolError('response body was not valid UTF-8', { cause });
  }
}

async function readBytes(response: Response): Promise<Uint8Array> {
  const declared = contentLength(response);
  if (declared !== undefined && declared > MAX_RESPONSE_BYTES) {
    throw new ProtocolError(`response exceeds the ${String(MAX_RESPONSE_BYTES)}-byte boundary`);
  }
  if (response.body === null) {
    return new Uint8Array();
  }

  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) {
        break;
      }
      size += value.byteLength;
      if (size > MAX_RESPONSE_BYTES) {
        await reader.cancel();
        throw new ProtocolError(`response exceeds the ${String(MAX_RESPONSE_BYTES)}-byte boundary`);
      }
      chunks.push(value);
    }
  } finally {
    reader.releaseLock();
  }

  const body = new Uint8Array(size);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return body;
}

function contentLength(response: Response): number | undefined {
  const raw = response.headers.get('content-length');
  if (raw === null) {
    return undefined;
  }
  const value = Number(raw);
  return Number.isSafeInteger(value) && value >= 0 ? value : undefined;
}

function nonEmptyText(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() !== '' ? value : undefined;
}

function nonNegativeInteger(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 ? value : undefined;
}

function messageOf(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
