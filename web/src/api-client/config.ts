import {
  CredentialError,
  RequestIdError,
  ResponseObserverError,
  TransportError,
  ValidationError,
} from './errors.js';

/** A bearer token resolved for each request and reconnect attempt. */
export type Credential = () => string | undefined | Promise<string | undefined>;

/** A correlation id resolved once for each caller-visible operation. */
export type RequestIdProvider = () => string | Promise<string>;

/** Headers available as soon as an HTTP response has completed its handshake. */
export interface ResponseMetadata {
  readonly status: number;
  readonly requestId?: string;
}

/** Observe every HTTP response before the SDK decodes its status or body. */
export type ResponseObserver = (metadata: ResponseMetadata) => void | Promise<void>;

export interface ClientConfig {
  /** Deployment root. Empty means a same-origin deployment. */
  readonly baseUrl?: string;
  /** Bearer token provider. It is called for every attempt so rotation is visible. */
  readonly credential?: Credential;
  /** Whether requests may carry browser cookies. Defaults to true. */
  readonly cookies?: boolean;
  /** Fetch implementation for browsers, Node, tests, and wrapped transports. */
  readonly fetch?: typeof globalThis.fetch;
  /** Default deadline for one request or stream handshake. */
  readonly requestTimeoutMs?: number;
  /** Correlation-id provider. Defaults to a locally generated UUID. */
  readonly requestIdProvider?: RequestIdProvider;
  /** Called once for every received HTTP response, before status or body decoding. */
  readonly responseObserver?: ResponseObserver;
}

export interface ResolvedConfig {
  readonly baseUrl: string;
  readonly credential?: Credential;
  readonly cookies: boolean;
  readonly fetch: typeof globalThis.fetch;
  readonly requestTimeoutMs: number;
  readonly requestIdProvider: RequestIdProvider;
  readonly responseObserver?: ResponseObserver;
}

export const DEFAULT_REQUEST_TIMEOUT_MS = 30_000;

export function resolveConfig(config: ClientConfig = {}): ResolvedConfig {
  const timeout = config.requestTimeoutMs ?? DEFAULT_REQUEST_TIMEOUT_MS;
  if (!Number.isSafeInteger(timeout) || timeout <= 0) {
    throw new ValidationError('requestTimeoutMs must be a positive safe integer');
  }
  return {
    baseUrl: normaliseBaseUrl(config.baseUrl ?? ''),
    credential: config.credential,
    cookies: config.cookies ?? true,
    fetch: config.fetch ?? platformFetch,
    requestTimeoutMs: timeout,
    requestIdProvider: config.requestIdProvider ?? generateRequestId,
    responseObserver: config.responseObserver,
  };
}

function normaliseBaseUrl(value: string): string {
  const base = value.replace(/\/+$/, '');
  if (base === '' || base.startsWith('/')) {
    return base;
  }

  let parsed: URL;
  try {
    parsed = new URL(base);
  } catch (cause) {
    throw new ValidationError('baseUrl must be an HTTP URL or a root-relative path', { cause });
  }
  if (!['http:', 'https:'].includes(parsed.protocol)) {
    throw new ValidationError('baseUrl must use HTTP or HTTPS');
  }
  if (parsed.username !== '' || parsed.password !== '') {
    throw new ValidationError('baseUrl must not contain user information');
  }
  if (parsed.search !== '' || parsed.hash !== '') {
    throw new ValidationError('baseUrl must not contain a query or fragment');
  }
  return base;
}

const platformFetch: typeof globalThis.fetch = (input, init) => {
  const implementation = globalThis.fetch;
  if (typeof implementation !== 'function') {
    throw new TransportError('no fetch is available; inject one through ClientConfig.fetch');
  }
  return implementation.call(globalThis, input, init);
};

export function urlOf(config: ResolvedConfig, path: string): string {
  if (!path.startsWith('/')) {
    throw new ValidationError('request paths must begin with a slash');
  }
  return `${config.baseUrl}${path}`;
}

export async function headersFor(
  config: ResolvedConfig,
  extra?: HeadersInit,
  options: { readonly authenticate?: boolean; readonly requestId?: string } = {},
): Promise<Record<string, string>> {
  const headers = new Headers({ 'Content-Type': 'application/json' });
  for (const [key, value] of new Headers(extra ?? {})) {
    headers.set(key, value);
  }
  if (options.requestId !== undefined) {
    headers.set('X-Request-Id', options.requestId);
  }
  if (options.authenticate === false) {
    return Object.fromEntries(headers);
  }
  let token: string | undefined;
  try {
    token = await config.credential?.();
  } catch (cause) {
    throw new CredentialError('the credential provider failed', { cause });
  }
  if (token !== undefined) {
    if (token === '') {
      throw new CredentialError('the credential provider returned an empty bearer token');
    }
    try {
      headers.set('Authorization', `Bearer ${token}`);
    } catch (cause) {
      throw new CredentialError('the credential is not valid in an HTTP bearer header', { cause });
    }
  }
  return Object.fromEntries(headers);
}

/** Resolve and validate one operation's caller-visible correlation id. */
export async function resolveRequestId(
  config: ResolvedConfig,
  requested?: string,
): Promise<string> {
  if (requested !== undefined) {
    return validRequestId(requested, (message, options) => new ValidationError(message, options));
  }
  let resolved: string;
  try {
    resolved = await config.requestIdProvider();
  } catch (cause) {
    throw new RequestIdError('the request-id provider failed', { cause });
  }
  return validRequestId(resolved, (message, options) => new RequestIdError(message, options));
}

/** Notify the configured observer without exposing or consuming the response body. */
export async function observeResponse(
  config: ResolvedConfig,
  response: Response,
): Promise<ResponseMetadata> {
  const requestId = responseRequestId(response);
  const metadata: ResponseMetadata = {
    status: response.status,
    ...(requestId === undefined ? {} : { requestId }),
  };
  try {
    await config.responseObserver?.(metadata);
  } catch (cause) {
    throw new ResponseObserverError({ cause });
  }
  return metadata;
}

/** Read a usable response correlation id; old or malformed intermediaries act as absent. */
export function responseRequestId(response: Response): string | undefined {
  let raw: string | null;
  try {
    raw = response.headers.get('x-request-id');
  } catch {
    return undefined;
  }
  if (raw === null || raw.trim() === '' || !isVisibleAscii(raw)) {
    return undefined;
  }
  try {
    const headers = new Headers({ 'X-Request-Id': raw });
    return headers.get('x-request-id') ?? undefined;
  } catch {
    return undefined;
  }
}

function validRequestId(
  value: unknown,
  error: (message: string, options?: ErrorOptions) => Error,
): string {
  if (typeof value !== 'string' || value.trim() === '' || !isVisibleAscii(value)) {
    throw error('the request id must be a non-empty HTTP header value');
  }
  try {
    const headers = new Headers({ 'X-Request-Id': value });
    const normalized = headers.get('x-request-id');
    if (normalized === null || normalized.trim() === '') {
      throw error('the request id must be a non-empty HTTP header value');
    }
    if (normalized !== value) {
      throw error('the request id must not require HTTP header normalization');
    }
    return value;
  } catch (cause) {
    if (cause instanceof ValidationError || cause instanceof RequestIdError) {
      throw cause;
    }
    throw error('the request id is not valid in an HTTP header', { cause });
  }
}

function isVisibleAscii(value: string): boolean {
  return [...value].every((character) => {
    const code = character.charCodeAt(0);
    return code >= 0x20 && code <= 0x7e;
  });
}

function generateRequestId(): string {
  if (typeof globalThis.crypto?.randomUUID === 'function') {
    return globalThis.crypto.randomUUID();
  }
  const bytes = new Uint8Array(16);
  if (typeof globalThis.crypto?.getRandomValues === 'function') {
    globalThis.crypto.getRandomValues(bytes);
  } else {
    for (let index = 0; index < bytes.length; index += 1) {
      bytes[index] = Math.floor(Math.random() * 256);
    }
  }
  bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x40;
  bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80;
  const hex = [...bytes].map((byte) => byte.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}
