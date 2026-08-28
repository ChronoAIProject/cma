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
const DEFAULT_MAX_PAGES = 100;
const DEFAULT_MAX_ITEMS = 20_000;

/** Smallest page a bounded collection accepts. */
export const MIN_PAGE_SIZE = 1;

/** Largest page a bounded collection accepts. */
export const MAX_PAGE_SIZE = 200;

/** Deadline and cancellation controls shared by every operation. */
export interface OperationOptions {
  readonly signal?: AbortSignal;
  readonly timeoutMs?: number;
  /** Correlation id for this complete operation, including pages and reconnects. */
  readonly requestId?: string;
}

export interface RequestOptions extends OperationOptions {
  readonly method?: string;
  readonly body?: unknown;
  readonly headers?: HeadersInit;
  readonly expectedStatus?: number | readonly number[];
}

declare const continuationBrand: unique symbol;

/** Opaque, route-bound target for the next page returned by CMA. */
export type PageContinuation = string & { readonly [continuationBrand]: true };

/** Caller-controlled position within one bounded collection. */
export interface PageRequest {
  readonly limit?: number;
  readonly cursor?: string;
  readonly next?: PageContinuation;
}

/** One bounded collection response. */
export interface Page<T> {
  readonly items: readonly T[];
  readonly next?: PageContinuation;
}

/** Resource ceilings for a compatibility operation that aggregates every page. */
export interface PageWalkOptions extends OperationOptions {
  /** Maximum responses to accept before refusing an unexpectedly long walk. */
  readonly maxPages?: number;
  /** Maximum decoded rows to retain across the complete operation. */
  readonly maxItems?: number;
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

/** Make one JSON request and validate that its body is JSON. */
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
    const text = await readText(response, signal);
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

/**
 * Build a bounded collection target without interpreting an opaque continuation.
 *
 * A continuation is already a complete route-bound target and therefore cannot
 * be combined with first-page filters. The server owns everything inside it.
 */
export function pageTarget(
  collectionPath: string,
  page: PageRequest = {},
  filters: Readonly<Record<string, string | undefined>> = {},
): string {
  validateCollectionPath(collectionPath);
  const hasFilters = Object.values(filters).some((value) => value !== undefined);
  if (page.next !== undefined) {
    if (page.limit !== undefined || page.cursor !== undefined || hasFilters) {
      throw new ValidationError('a pagination continuation cannot be combined with page filters');
    }
    return validatePageTarget(page.next, collectionPath);
  }

  const params = new URLSearchParams();
  for (const [name, value] of Object.entries(filters)) {
    if (value !== undefined) {
      params.set(name, value);
    }
  }
  if (page.limit !== undefined) {
    if (
      !Number.isSafeInteger(page.limit) ||
      page.limit < MIN_PAGE_SIZE ||
      page.limit > MAX_PAGE_SIZE
    ) {
      throw new ValidationError(
        `page limit must be an integer from ${String(MIN_PAGE_SIZE)} through ${String(MAX_PAGE_SIZE)}`,
      );
    }
    params.set('limit', String(page.limit));
  }
  if (page.cursor !== undefined) {
    if (page.cursor === '') {
      throw new ValidationError('page cursor must not be empty');
    }
    params.set('cursor', page.cursor);
  }
  const query = params.toString();
  return query === '' ? collectionPath : `${collectionPath}?${query}`;
}

/** Fetch and validate exactly one bounded collection page. */
export async function requestPage<T>(
  config: ResolvedConfig,
  collectionPath: string,
  target: string,
  decodePage: (body: unknown) => readonly T[],
  options: OperationOptions = {},
): Promise<Page<T>> {
  validateCollectionPath(collectionPath);
  const checkedTarget = validatePageTarget(target, collectionPath);
  return withDeadline(config, options, (signal, requestId) =>
    fetchPage(config, collectionPath, checkedTarget, decodePage, signal, requestId),
  );
}

/** Walk one array collection through the server's RFC 8288 next links. */
export async function requestPages<T>(
  config: ResolvedConfig,
  path: string,
  decodePage: (body: unknown) => readonly T[],
  options: PageWalkOptions = {},
): Promise<readonly T[]> {
  const collectionPath = path.split('?', 1)[0];
  if (collectionPath === undefined) {
    throw new ValidationError('paginated request paths must begin with a slash');
  }
  validateCollectionPath(collectionPath);
  const first = validatePageTarget(path, collectionPath);
  const maxPages = walkBoundary(options.maxPages, DEFAULT_MAX_PAGES, 'maxPages');
  const maxItems = walkBoundary(options.maxItems, DEFAULT_MAX_ITEMS, 'maxItems');

  return withDeadline(config, options, async (signal, requestId) => {
    const visited = new Set<string>();
    const items: T[] = [];
    let pages = 0;
    let next: string | undefined = first;
    while (next !== undefined) {
      if (pages >= maxPages) {
        throw new ProtocolError(`pagination exceeded its ${String(maxPages)}-page boundary`);
      }
      if (visited.has(next)) {
        throw new ProtocolError('pagination repeated a continuation link');
      }
      visited.add(next);
      const page: Page<T> = await fetchPage(
        config,
        collectionPath,
        next,
        decodePage,
        signal,
        requestId,
      );
      pages += 1;
      if (page.items.length > maxItems - items.length) {
        throw new ProtocolError(`pagination exceeded its ${String(maxItems)}-item boundary`);
      }
      items.push(...page.items);
      next = page.next;
    }
    return items;
  });
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
    const text = await readText(response, signal);
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
  return withDeadline(config, options, async (signal, requestId) => {
    const { response, metadata } = await send(config, path, options, signal, requestId);
    await requireSuccess(response, options.expectedStatus, signal, metadata.requestId);
    return readBytes(response, signal);
  });
}

/** Execute a fetch-backed operation under one caller-visible deadline. */
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

/** Race one async boundary against a transport signal without starting it after abort. */
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

/** Stop a losing async hook before it can advance to the next transport stage. */
export function throwIfSignalAborted(signal: AbortSignal): void {
  if (signal.aborted) {
    throw abortError(signal);
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
  requestId: string,
): Promise<CompletedResponse> {
  throwIfSignalAborted(signal);
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
  const headers = await headersFor(config, options.headers, { requestId });
  throwIfSignalAborted(signal);
  const response = await config.fetch(urlOf(config, path), {
    method: options.method ?? 'GET',
    headers,
    credentials: config.cookies ? 'include' : 'omit',
    body,
    signal,
  });
  throwIfSignalAborted(signal);
  const metadata = await observeResponse(config, response);
  throwIfSignalAborted(signal);
  return { response, metadata };
}

async function responseJson(response: Response, signal: AbortSignal): Promise<unknown> {
  const text = await readText(response, signal);
  if (text.trim() === '') {
    return null;
  }
  try {
    return JSON.parse(text) as unknown;
  } catch (cause) {
    throw new ProtocolError('success response was not valid JSON', { cause });
  }
}

function nextPagePath(link: string | null, collectionPath: string): PageContinuation | undefined {
  if (link === null) {
    return undefined;
  }
  for (const entry of link.split(',')) {
    const match = /^\s*<([^>]*)>(.*)$/.exec(entry);
    if (match === null) {
      if (linkRelations(entry)?.includes('next') === true) {
        throw new ProtocolError('pagination next link was malformed');
      }
      continue;
    }
    const target = match[1];
    const parameters = match[2] ?? '';
    if (linkRelations(parameters)?.includes('next') !== true) {
      continue;
    }
    if (target === undefined || !target.startsWith(`${collectionPath}?`) || target.includes('#')) {
      throw new ProtocolError('pagination next link left its collection');
    }
    return target as PageContinuation;
  }
  return undefined;
}

function linkRelations(parameters: string): readonly string[] | undefined {
  const marker = /(?:^|;)\s*rel\s*=/i.exec(parameters);
  if (marker === null) {
    return undefined;
  }
  const relation = /(?:^|;)\s*rel\s*=\s*(?:"([^"]*)"|([^";\s]+))\s*(?=;|$)/i.exec(parameters);
  if (relation === null) {
    throw new ProtocolError('pagination Link header carried a malformed relation');
  }
  return (relation[1] ?? relation[2] ?? '')
    .split(/\s+/)
    .filter((name) => name !== '')
    .map((name) => name.toLowerCase());
}

async function requireSuccess(
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
  const retryAfterMs = retryAfterMilliseconds(response);
  if (detail === undefined) {
    return new ProtocolError(`HTTP ${String(response.status)} problem document carried no detail`);
  }

  if (code === 'capability_unsupported') {
    const capability = nonEmptyText(body.capability);
    return capability === undefined
      ? new ProtocolError('capability_unsupported problem omitted its typed capability field')
      : new CapabilityRefused(response.status, detail, capability, requestId, retryAfterMs);
  }
  if (code === 'session_limit_reached') {
    return new SessionLimitReached(
      response.status,
      detail,
      nonNegativeInteger(body.limit),
      nonNegativeInteger(body.live),
      requestId,
      retryAfterMs,
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
      requestId,
      retryAfterMs,
    );
  }
  return new ApiError(response.status, detail, code, requestId, retryAfterMs);
}

const MAX_RETRY_AFTER_MS = 2_147_483_647;

/** Parse only a positive RFC Retry-After value that a JavaScript timer can represent. */
function retryAfterMilliseconds(response: Response): number | undefined {
  const value = response.headers.get('retry-after')?.trim();
  if (value === undefined || value === '') return undefined;
  if (/^[0-9]+$/.test(value)) {
    const seconds = Number(value);
    const milliseconds = seconds * 1_000;
    return Number.isSafeInteger(milliseconds) &&
      milliseconds > 0 &&
      milliseconds <= MAX_RETRY_AFTER_MS
      ? milliseconds
      : undefined;
  }
  const deadline = Date.parse(value);
  const milliseconds = Math.ceil(deadline - Date.now());
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

async function readBytes(response: Response, signal?: AbortSignal): Promise<Uint8Array> {
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
      const { done, value } = await readChunk(reader, signal);
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

async function fetchPage<T>(
  config: ResolvedConfig,
  collectionPath: string,
  target: string,
  decodePage: (body: unknown) => readonly T[],
  signal: AbortSignal,
  requestId: string,
): Promise<Page<T>> {
  const { response, metadata } = await send(
    config,
    target,
    { expectedStatus: 200 },
    signal,
    requestId,
  );
  await requireSuccess(response, 200, signal, metadata.requestId);
  const items = decodePage(await responseJson(response, signal));
  if (items.length > MAX_PAGE_SIZE) {
    throw new ProtocolError(
      `pagination page exceeded the ${String(MAX_PAGE_SIZE)}-item server boundary`,
    );
  }
  const next = nextPagePath(response.headers.get('link'), collectionPath);
  return next === undefined ? { items } : { items, next };
}

function validateCollectionPath(path: string): void {
  if (!path.startsWith('/') || path.startsWith('//') || path.includes('?') || path.includes('#')) {
    throw new ValidationError('paginated collection paths must be absolute paths without a query');
  }
}

function validatePageTarget(target: string, collectionPath: string): string {
  if (
    (target !== collectionPath && !target.startsWith(`${collectionPath}?`)) ||
    target.includes('#')
  ) {
    throw new ValidationError('pagination target left its collection');
  }
  return target;
}

function walkBoundary(value: number | undefined, fallback: number, name: string): number {
  const chosen = value ?? fallback;
  if (!Number.isSafeInteger(chosen) || chosen <= 0) {
    throw new ValidationError(`${name} must be a positive safe integer`);
  }
  return chosen;
}

function abortError(signal: AbortSignal): Error {
  return signal.reason instanceof Error
    ? signal.reason
    : new DOMException('the operation was aborted', 'AbortError');
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
