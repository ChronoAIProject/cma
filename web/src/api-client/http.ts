import {
  headersFor,
  observeResponse,
  resolveRequestId,
  responseRequestId,
  urlOf,
  type ResolvedConfig,
  type ResponseMetadata,
} from './config.js';
import {
  ApiError,
  CmaError,
  ProtocolError,
  RequestAborted,
  TimeoutError,
  TransportError,
  ValidationError,
} from './errors.js';
import { MAX_REVOCATION_LINK_LENGTH } from './resource-revocation-links.js';

const MAX_RESPONSE_BYTES = 8 * 1024 * 1024;

/** Deadline and cancellation controls shared by every operation. */
export interface OperationOptions {
  readonly signal?: AbortSignal;
  readonly timeoutMs?: number;
  /** Correlation id for the complete caller-visible operation. */
  readonly requestId?: string;
}

/** Options for one HTTP request. */
export interface RequestOptions extends OperationOptions {
  readonly cache?: RequestCache;
  readonly redirect?: RequestRedirect;
  readonly method?: string;
  readonly body?: unknown;
  /** Binary body for upload endpoints; mutually exclusive with JSON body. */
  readonly rawBody?: Blob;
  readonly headers?: HeadersInit;
  readonly expectedStatus?: number | readonly number[];
}

interface CompletedResponse {
  readonly response: Response;
  readonly metadata: ResponseMetadata;
}

/** Open and validate an HTTP response without consuming its successful body. */
export async function requestResponse(
  config: ResolvedConfig,
  path: string,
  options: RequestOptions = {},
): Promise<Response> {
  return withDeadline(config, options, async (signal, requestId) => {
    const { response, metadata } = await send(config, path, options, signal, requestId);
    await requireSuccess(response, options.expectedStatus, signal, metadata.requestId);
    return response;
  });
}

/** Make one request whose successful body is JSON. */
export async function request(
  config: ResolvedConfig,
  path: string,
  options: RequestOptions = {},
): Promise<unknown> {
  return withDeadline(config, options, async (signal, requestId) => {
    const { response, metadata } = await send(config, path, options, signal, requestId);
    await requireSuccess(response, options.expectedStatus, signal, metadata.requestId);
    if (response.status === 204) {
      return null;
    }
    return readJsonBody(response, signal);
  });
}

/** Bounded JSON decoding shared with non-problem browser authentication responses. */
export async function readJsonBody(response: Response, signal?: AbortSignal): Promise<unknown> {
  const body = await readText(response, signal);
  if (body.trim() === '') return null;
  try {
    return JSON.parse(body) as unknown;
  } catch (cause) {
    throw new ProtocolError('response was not valid JSON', { cause });
  }
}

/** Make one request whose successful response must not carry a body. */
export async function requestVoid(
  config: ResolvedConfig,
  path: string,
  options: RequestOptions = {},
): Promise<void> {
  await withDeadline(config, options, async (signal, requestId) => {
    const { response, metadata } = await send(config, path, options, signal, requestId);
    await requireSuccess(response, options.expectedStatus, signal, metadata.requestId);
    const body = await readText(response, signal);
    if (body.trim() !== '') {
      throw new ProtocolError(`HTTP ${String(response.status)} unexpectedly carried a body`);
    }
  });
}

/** Make one request whose successful body is opaque bytes. */
export async function requestBytes(
  config: ResolvedConfig,
  path: string,
  options: RequestOptions = {},
  maxBytes = MAX_RESPONSE_BYTES,
): Promise<Uint8Array> {
  return withDeadline(config, options, async (signal, requestId) => {
    const { response, metadata } = await send(config, path, options, signal, requestId);
    await requireSuccess(response, options.expectedStatus, signal, metadata.requestId);
    return readBytes(response, signal, maxBytes);
  });
}

/** Execute a request under one caller-visible deadline and correlation id. */
export async function withDeadline<T>(
  config: ResolvedConfig,
  options: OperationOptions,
  operation: (signal: AbortSignal, requestId: string) => Promise<T>,
): Promise<T> {
  return withOperationDeadline(config, options, async (signal) => {
    const requestId = await resolveRequestId(config, options.requestId);
    throwIfSignalAborted(signal);
    return operation(signal, requestId);
  });
}

/** Execute callback and transport work under one caller-visible deadline. */
export async function withOperationDeadline<T>(
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
    const value = await raceWithAbortSignal(controller.signal, () => operation(controller.signal));
    throwIfSignalAborted(controller.signal);
    return value;
  } catch (cause) {
    if (isAborted(options.signal)) {
      throw new RequestAborted(undefined, { cause });
    }
    if (timedOut) {
      throw new TimeoutError(timeoutMs, { cause });
    }
    if (cause instanceof CmaError) {
      throw cause;
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

/** Race one asynchronous boundary against an abort signal. */
export async function raceWithAbortSignal<T>(
  signal: AbortSignal,
  operation: () => Promise<T>,
): Promise<T> {
  throwIfSignalAborted(signal);
  let onAbort: (() => void) | undefined;
  const aborted = new Promise<never>((_resolve, reject) => {
    onAbort = () => reject(abortError(signal));
    signal.addEventListener('abort', onAbort, { once: true });
  });
  try {
    return await Promise.race([
      Promise.resolve().then(() => {
        throwIfSignalAborted(signal);
        return operation();
      }),
      aborted,
    ]);
  } finally {
    if (onAbort !== undefined) {
      signal.removeEventListener('abort', onAbort);
    }
  }
}

/** Stop work before it can cross another boundary after cancellation. */
export function throwIfSignalAborted(signal: AbortSignal): void {
  if (signal.aborted) {
    throw abortError(signal);
  }
}

export async function send(
  config: ResolvedConfig,
  path: string,
  options: RequestOptions,
  signal: AbortSignal,
  requestId: string,
): Promise<CompletedResponse> {
  throwIfSignalAborted(signal);
  if (options.body !== undefined && options.rawBody !== undefined) {
    throw new ValidationError('request cannot contain both JSON and binary bodies');
  }
  let body: string | Blob | undefined = options.rawBody;
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
  const headers = await headersFor(config, options.headers, { requestId });
  throwIfSignalAborted(signal);
  const response = await config.fetch(urlOf(config, path), {
    method: options.method ?? 'GET',
    headers,
    credentials: config.cookies ? 'include' : 'omit',
    body,
    signal,
    ...(options.cache === undefined ? {} : { cache: options.cache }),
    ...(options.redirect === undefined ? {} : { redirect: options.redirect }),
  });
  throwIfSignalAborted(signal);
  const metadata = await observeResponse(config, response);
  throwIfSignalAborted(signal);
  return { response, metadata };
}

export async function requireSuccess(
  response: Response,
  expected: number | readonly number[] | undefined,
  signal?: AbortSignal,
  requestId?: string,
): Promise<void> {
  if (!response.ok) {
    throw await errorFromResponse(response, signal, requestId);
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
export async function errorFromResponse(
  response: Response,
  signal?: AbortSignal,
  requestId = responseRequestId(response),
): Promise<ApiError | ProtocolError> {
  const text = await readText(response, signal);
  let raw: unknown;
  try {
    raw = JSON.parse(text);
  } catch (cause) {
    return new ProtocolError(
      `HTTP ${String(response.status)} did not carry a machine-readable problem document`,
      { cause },
    );
  }
  return errorFromBody(response, raw, requestId);
}

/** Decode an already bounded JSON body while retaining HTTP correlation and retry metadata. */
export function errorFromBody(
  response: Response,
  raw: unknown,
  requestId = responseRequestId(response),
): ApiError | ProtocolError {
  if (typeof raw !== 'object' || raw === null || Array.isArray(raw)) {
    return new ProtocolError(`HTTP ${String(response.status)} problem document was not an object`);
  }

  const body = raw as Record<string, unknown>;
  if (typeof body.status === 'number' && body.status !== response.status) {
    return new ProtocolError(
      `problem status ${String(body.status)} did not match HTTP ${String(response.status)}`,
    );
  }
  if (
    'error' in body &&
    nonEmptyText(body.detail) === undefined &&
    nonEmptyText(body.title) === undefined
  ) {
    const error = body.error;
    if (
      typeof error === 'string' &&
      Object.keys(body).length === 1 &&
      [
        'invalid_media_continuation',
        'media_continuation_refused',
        'media_continuation_conflict',
        'media_attachment_not_found',
        'media_agent_not_ready',
        'media_too_large',
        'media_continuation_unavailable',
        'media_upload_capacity',
      ].includes(error)
    )
      return new ApiError(
        response.status,
        error,
        error,
        requestId,
        retryAfterMilliseconds(response),
      );
    if (typeof error !== 'object' || error === null || Array.isArray(error))
      return new ProtocolError('Responses error envelope is invalid');
    const value = error as Record<string, unknown>;
    const message = nonEmptyText(value.message);
    const code = nonEmptyText(value.code);
    if (
      message === undefined ||
      code === undefined ||
      nonEmptyText(value.type) === undefined ||
      (value.param !== null && typeof value.param !== 'string')
    )
      return new ProtocolError('Responses error fields are invalid');
    return new ApiError(
      response.status,
      message,
      code,
      requestId,
      retryAfterMilliseconds(response),
      Object.freeze({ type: value.type, param: value.param }),
    );
  }
  const code = nonEmptyText(body.code);
  const detail = nonEmptyText(body.detail) ?? nonEmptyText(body.title);
  if (detail === undefined) {
    return new ProtocolError(`HTTP ${String(response.status)} problem document carried no detail`);
  }
  const standard = new Set(['type', 'title', 'status', 'detail', 'instance', 'code']);
  const extensions = Object.freeze(
    Object.fromEntries(Object.entries(body).filter(([key]) => !standard.has(key))),
  );
  const statusLink =
    code === 'resource_revocation_pending' && response.status === 409
      ? response.headers.get('link')
      : null;
  return new ApiError(
    response.status,
    detail,
    code,
    requestId,
    retryAfterMilliseconds(response),
    extensions,
    statusLink !== null && statusLink.length <= MAX_REVOCATION_LINK_LENGTH ? statusLink : undefined,
    nonEmptyText(body.type),
  );
}

const MAX_RETRY_AFTER_MS = 2_147_483_647;

export function retryAfterMilliseconds(response: Response): number | undefined {
  const value = response.headers.get('retry-after')?.trim();
  if (value === undefined || value === '') return undefined;
  if (/^[0-9]+$/.test(value)) {
    const milliseconds = Number(value) * 1_000;
    return Number.isSafeInteger(milliseconds) &&
      milliseconds > 0 &&
      milliseconds <= MAX_RETRY_AFTER_MS
      ? milliseconds
      : undefined;
  }
  const milliseconds = Math.ceil(Date.parse(value) - Date.now());
  return Number.isFinite(milliseconds) && milliseconds > 0 && milliseconds <= MAX_RETRY_AFTER_MS
    ? milliseconds
    : undefined;
}

async function readText(response: Response, signal?: AbortSignal): Promise<string> {
  const bytes = await readBytes(response, signal);
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes);
  } catch (cause) {
    throw new ProtocolError('response body was not valid UTF-8', { cause });
  }
}

export async function readBytes(
  response: Response,
  signal?: AbortSignal,
  maxBytes = MAX_RESPONSE_BYTES,
): Promise<Uint8Array> {
  if (!Number.isSafeInteger(maxBytes) || maxBytes <= 0)
    throw new ValidationError('response byte limit must be a positive safe integer');
  const declared = contentLength(response);
  if (declared !== undefined && declared > maxBytes) {
    throw new ProtocolError(`response exceeds the ${String(maxBytes)}-byte boundary`);
  }
  if (response.body === null) {
    return new Uint8Array();
  }

  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let size = 0;
  try {
    while (true) {
      const { done, value } = await readChunk(reader, signal);
      if (done) break;
      size += value.byteLength;
      if (size > maxBytes) {
        await reader.cancel();
        throw new ProtocolError(`response exceeds the ${String(maxBytes)}-byte boundary`);
      }
      chunks.push(value);
    }
  } finally {
    if (signal?.aborted === true) {
      await reader.cancel(signal.reason).catch(() => undefined);
    }
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

async function readChunk(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  signal: AbortSignal | undefined,
): Promise<ReadableStreamReadResult<Uint8Array>> {
  if (signal === undefined) {
    return reader.read();
  }
  if (signal.aborted) {
    throw abortError(signal);
  }
  return new Promise((resolve, reject) => {
    let settled = false;
    const finish = (complete: () => void) => {
      if (!settled) {
        settled = true;
        signal.removeEventListener('abort', onAbort);
        complete();
      }
    };
    const onAbort = () => finish(() => reject(abortError(signal)));
    signal.addEventListener('abort', onAbort, { once: true });
    reader.read().then(
      (result) => finish(() => resolve(result)),
      (cause: unknown) => finish(() => reject(cause)),
    );
  });
}

function abortError(signal: AbortSignal): Error {
  return signal.reason instanceof Error
    ? signal.reason
    : new DOMException('the operation was aborted', 'AbortError');
}

function contentLength(response: Response): number | undefined {
  const raw = response.headers.get('content-length');
  if (raw === null) return undefined;
  const value = Number(raw);
  return Number.isSafeInteger(value) && value >= 0 ? value : undefined;
}

function nonEmptyText(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim() !== '' ? value : undefined;
}

function messageOf(cause: unknown): string {
  return cause instanceof Error ? cause.message : String(cause);
}
